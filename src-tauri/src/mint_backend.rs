use anyhow::{anyhow, Context, Result};
use move_core_types::{parser::parse_transaction_argument, u256::U256};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use sui_ptb_helper::{
    move_call, MoveTarget, ObjectAccess, ProgrammableTransactionBuilder, SuiPtbClient, SuiPtbConfig,
};
use sui_verifier_api::artifact::DEFAULT_CHUNK_SIZE;
use sui_verifier_api::artifact_ptb::ArtifactPtb;
use zkmove_cli::{
    api,
    common::{
        get_circuit_config_args_from_move_toml, get_entry_call_from_move_toml, load_package,
        read_params, KZGVariant,
    },
};

const SUI_MAX_TRANSACTION_BYTES: usize = 128 * 1024;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MintManifest {
    app_package: String,
    module: String,
    function: String,
    store: String,
    verifier_api_package: String,
    params_object_id: String,
    vk_object_id: String,
    gas_budget: String,
    circuit: CircuitManifest,
    #[serde(default)]
    sui: SuiManifest,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SuiManifest {
    rpc_url: Option<String>,
    keystore_path: Option<PathBuf>,
    sender: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CircuitManifest {
    package_path: PathBuf,
    entry_function: String,
    circuit_name: String,
    pubs_indices: Vec<usize>,
    kzg: String,
    k: Option<u32>,
    srs_path: PathBuf,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProveResult {
    pub encrypted_value: String,
    pub proof_bytes: usize,
    pub instance_bytes: usize,
    pub k: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MintResult {
    pub encrypted_value: String,
    pub proof_bytes: usize,
    pub instance_bytes: usize,
    pub k: u32,
    pub proof_builder: Option<String>,
    pub proof_digest: Vec<u8>,
    pub tx_digest: String,
    pub tx_mode: String,
    pub store_balance: String,
}

pub struct MintProof {
    pub encrypted_value: String,
    proof: Vec<u8>,
    instance: Vec<u8>,
    variant: KZGVariant,
    pub k: u32,
}

pub struct MintTx {
    pub proof_builder: Option<String>,
    pub proof_digest: Vec<u8>,
    pub tx_digest: String,
    pub tx_mode: String,
}

pub fn manifest_path() -> PathBuf {
    std::env::var("ZKMOVE_MINT_MANIFEST")
        .map(PathBuf::from)
        .unwrap_or_else(|_| Path::new(env!("CARGO_MANIFEST_DIR")).join("../mint-min.manifest.json"))
}

pub fn load_manifest() -> Result<MintManifest> {
    load_manifest_from_path(manifest_path())
}

pub fn load_manifest_from_path(path: PathBuf) -> Result<MintManifest> {
    let bytes = std::fs::read(&path)
        .with_context(|| format!("failed to read manifest {}", path.display()))?;
    let mut manifest: MintManifest = serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse manifest {}", path.display()))?;
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    manifest.circuit.package_path = resolve_manifest_path(
        base_dir,
        "ZKMOVE_MINT_PACKAGE_PATH",
        &manifest.circuit.package_path,
    );
    manifest.circuit.srs_path =
        resolve_manifest_path(base_dir, "ZKMOVE_MINT_SRS_PATH", &manifest.circuit.srs_path);
    if let Some(keystore_path) = manifest.sui.keystore_path.as_ref() {
        if keystore_path.is_relative() {
            manifest.sui.keystore_path = Some(base_dir.join(keystore_path));
        }
    }
    Ok(manifest)
}

fn resolve_manifest_path(base_dir: &Path, env_var: &str, manifest_path: &Path) -> PathBuf {
    if let Ok(path) = std::env::var(env_var) {
        return PathBuf::from(path);
    }
    if manifest_path.is_relative() {
        base_dir.join(manifest_path)
    } else {
        manifest_path.to_path_buf()
    }
}

fn parse_u128_arg(name: &str, value: &str) -> Result<u128> {
    value
        .parse::<u128>()
        .with_context(|| format!("{name} must be a u128 decimal string"))
}

fn parse_kzg_variant(raw: &str) -> Result<KZGVariant> {
    match raw.to_ascii_lowercase().as_str() {
        "gwc" => Ok(KZGVariant::GWC),
        "shplonk" => Ok(KZGVariant::SHPLONK),
        other => Err(anyhow!("unsupported kzg variant: {other}")),
    }
}

fn kzg_variant_u8(variant: KZGVariant) -> u8 {
    match variant {
        KZGVariant::GWC => 0,
        KZGVariant::SHPLONK => 1,
    }
}

pub fn compute_encrypted_value(value: String, nonce: String) -> Result<String> {
    let value = parse_u128_arg("value", &value)?;
    let nonce = parse_u128_arg("nonce", &nonce)?;
    api::poseidon::poseidon_hash(value, nonce).map(|value| value.to_string())
}

pub fn prove_mint_sync(manifest: &MintManifest, value: &str, nonce: &str) -> Result<MintProof> {
    let value = parse_u128_arg("value", value)?;
    let nonce = parse_u128_arg("nonce", nonce)?;
    let encrypted_value = api::poseidon::poseidon_hash(value, nonce)?;
    let encrypted_value = encrypted_value.to_string();

    let package_path = &manifest.circuit.package_path;
    let manifest_path = package_path.join("Move.toml");
    let package = load_package(package_path).with_context(|| {
        format!(
            "failed to load compiled off-chain package at {}; run `move build` there first",
            package_path.display()
        )
    })?;
    let (module_id, function_name) =
        get_entry_call_from_move_toml(&manifest_path, Some(&manifest.circuit.circuit_name))?;
    if function_name.as_str() != manifest.circuit.entry_function {
        return Err(anyhow!(
            "manifest entryFunction {} does not match Move.toml circuit entry {}",
            manifest.circuit.entry_function,
            function_name
        ));
    }
    let circuit_config = get_circuit_config_args_from_move_toml(
        &manifest_path,
        Some(&manifest.circuit.circuit_name),
    )?;
    let args = vec![
        parse_transaction_argument(&format!("{value}u128"))?,
        parse_transaction_argument(&format!("{encrypted_value}u256"))?,
        parse_transaction_argument(&format!("{nonce}u128"))?,
    ];

    let traces = api::generate_witness(&package, &module_id, &function_name, &args)?;
    let params = read_params(&manifest.circuit.srs_path)?;
    let ctx = api::setup_with_witness(
        package,
        &traces,
        circuit_config,
        params,
        manifest.circuit.pubs_indices.clone(),
    )?;
    if let Some(manifest_k) = manifest.circuit.k {
        if manifest_k != ctx.k {
            return Err(anyhow!(
                "manifest circuit.k {} does not match setup ctx.k {}",
                manifest_k,
                ctx.k
            ));
        }
    }
    let variant = parse_kzg_variant(&manifest.circuit.kzg)?;
    let proof = api::prove::prove_with_witness(&ctx, &traces, variant)?;
    api::verify(&ctx, variant, &proof.proof, &proof.instance)?;

    Ok(MintProof {
        encrypted_value,
        proof: proof.proof,
        instance: proof.instance,
        variant,
        k: ctx.k,
    })
}

async fn prove_mint_blocking(
    manifest: MintManifest,
    value: String,
    nonce: String,
) -> Result<MintProof> {
    tauri::async_runtime::spawn_blocking(move || prove_mint_sync(&manifest, &value, &nonce))
        .await
        .map_err(|err| anyhow!("proof generation task failed: {err}"))?
}

pub async fn client_from_manifest(manifest: &MintManifest) -> Result<SuiPtbClient> {
    let rpc_url = std::env::var("SUI_RPC_URL")
        .ok()
        .or_else(|| manifest.sui.rpc_url.clone())
        .unwrap_or_else(|| "http://127.0.0.1:9000".to_string());
    let keystore_path = std::env::var("SUI_KEYSTORE_PATH")
        .ok()
        .map(PathBuf::from)
        .or_else(|| manifest.sui.keystore_path.clone());
    let sender = std::env::var("SUI_SENDER")
        .ok()
        .or_else(|| manifest.sui.sender.clone());
    let gas_budget = manifest
        .gas_budget
        .parse::<u64>()
        .context("gasBudget must be a u64 decimal string")?;

    SuiPtbClient::connect(SuiPtbConfig {
        rpc_url,
        keystore_path,
        sender,
        gas_budget,
    })
    .await
}

pub async fn prove_mint_value(value: String, nonce: String) -> Result<ProveResult> {
    let manifest = load_manifest()?;
    let proof = prove_mint_blocking(manifest, value, nonce).await?;
    Ok(ProveResult {
        encrypted_value: proof.encrypted_value,
        proof_bytes: proof.proof.len(),
        instance_bytes: proof.instance.len(),
        k: proof.k,
    })
}

pub async fn execute_mint_transaction(
    client: &SuiPtbClient,
    manifest: &MintManifest,
    proof: &MintProof,
    encrypted_value: &str,
) -> Result<MintTx> {
    if proof.proof.len() > SUI_MAX_TRANSACTION_BYTES {
        return Err(anyhow!(
            "proof is {} bytes, larger than the single PTB payload limit used by this MVP; non-atomic multi-transaction mint is intentionally disabled",
            proof.proof.len()
        ));
    }

    let mut ptb = ProgrammableTransactionBuilder::new();
    let store = client
        .input_object(&mut ptb, &manifest.store, ObjectAccess::Mutable)
        .await?;
    let params = client
        .input_object(
            &mut ptb,
            &manifest.params_object_id,
            ObjectAccess::Immutable,
        )
        .await?;
    let vk = client
        .input_object(&mut ptb, &manifest.vk_object_id, ObjectAccess::Immutable)
        .await?;
    let staged_proof = {
        let mut artifact_ptb = ArtifactPtb::new(&mut ptb, &manifest.verifier_api_package);
        artifact_ptb.stage_proof(proof.proof.clone(), Some(DEFAULT_CHUNK_SIZE))?
    };
    let proof_digest = ptb.pure(staged_proof.proof_digest.to_vec())?;
    let kzg_variant = ptb.pure(kzg_variant_u8(proof.variant))?;
    let k_present = ptb.pure(true)?;
    let k = ptb.pure(proof.k)?;
    let encrypted_amount = U256::from_str(encrypted_value)
        .with_context(|| format!("invalid encrypted u256: {encrypted_value}"))?;
    let encrypted_amount = ptb.pure(encrypted_amount)?;
    move_call(
        &mut ptb,
        MoveTarget::new(&manifest.app_package, &manifest.module, &manifest.function)?,
        vec![
            store,
            params,
            vk,
            staged_proof.proof_builder,
            proof_digest,
            kzg_variant,
            k_present,
            k,
            encrypted_amount,
        ],
    );

    let response = client.execute(ptb.finish()).await?;
    Ok(MintTx {
        proof_builder: None,
        proof_digest: staged_proof.proof_digest.to_vec(),
        tx_digest: response.digest,
        tx_mode: "single_ptb".to_string(),
    })
}

pub async fn read_store_balance(client: &SuiPtbClient, manifest: &MintManifest) -> Result<String> {
    client.read_object_field(&manifest.store, "balance").await
}

pub async fn mint_value(value: String, nonce: String) -> Result<MintResult> {
    let manifest = load_manifest()?;
    let proof = prove_mint_blocking(manifest.clone(), value, nonce).await?;
    let client = client_from_manifest(&manifest).await?;
    let mint_tx =
        execute_mint_transaction(&client, &manifest, &proof, &proof.encrypted_value).await?;
    let balance = read_store_balance(&client, &manifest).await?;

    Ok(MintResult {
        encrypted_value: proof.encrypted_value,
        proof_bytes: proof.proof.len(),
        instance_bytes: proof.instance.len(),
        k: proof.k,
        proof_builder: mint_tx.proof_builder,
        proof_digest: mint_tx.proof_digest,
        tx_digest: mint_tx.tx_digest,
        tx_mode: mint_tx.tx_mode,
        store_balance: balance,
    })
}
