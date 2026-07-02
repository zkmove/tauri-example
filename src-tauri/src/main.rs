#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use anyhow::{anyhow, Context, Result};
use move_core_types::{parser::parse_transaction_argument, u256::U256};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use sui_ptb_helper::{
    add_append_chunk_call, add_move_call, add_new_proof_builder_call, ChunkPlan,
    ProgrammableTransactionBuilder, DEFAULT_CHUNK_SIZE,
};
use sui_ptb_helper::{SuiPtbClient, SuiPtbConfig};
use zkmove_cli::{
    api,
    common::{
        get_circuit_config_args_from_move_toml, get_entry_call_from_move_toml, load_package,
        read_params, KZGVariant,
    },
};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MintManifest {
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
struct ProveResult {
    encrypted_value: String,
    proof_bytes: usize,
    instance_bytes: usize,
    k: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MintResult {
    encrypted_value: String,
    proof_bytes: usize,
    instance_bytes: usize,
    k: u32,
    proof_builder: Option<String>,
    proof_digest: Vec<u8>,
    tx_digest: String,
    tx_mode: String,
    store_balance: String,
}

struct MintProof {
    encrypted_value: String,
    proof: Vec<u8>,
    instance: Vec<u8>,
    variant: KZGVariant,
    k: u32,
}

struct MintTx {
    proof_builder: Option<String>,
    proof_digest: Vec<u8>,
    tx_digest: String,
    tx_mode: String,
}

fn manifest_path() -> PathBuf {
    std::env::var("ZKMOVE_MINT_MANIFEST")
        .map(PathBuf::from)
        .unwrap_or_else(|_| Path::new(env!("CARGO_MANIFEST_DIR")).join("../mint-min.manifest.json"))
}

fn load_manifest() -> Result<MintManifest> {
    let path = manifest_path();
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

fn prove_mint_inner(manifest: &MintManifest, value: &str, nonce: &str) -> Result<MintProof> {
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
    tauri::async_runtime::spawn_blocking(move || prove_mint_inner(&manifest, &value, &nonce))
        .await
        .map_err(|err| anyhow!("proof generation task failed: {err}"))?
}

async fn client_from_manifest(manifest: &MintManifest) -> Result<SuiPtbClient> {
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
        chunk_size: None,
    })
    .await
}

#[tauri::command(async)]
fn compute_encrypted(value: String, nonce: String) -> Result<String, String> {
    let value = parse_u128_arg("value", &value).map_err(|e| e.to_string())?;
    let nonce = parse_u128_arg("nonce", &nonce).map_err(|e| e.to_string())?;
    api::poseidon::poseidon_hash(value, nonce)
        .map(|value| value.to_string())
        .map_err(|e| e.to_string())
}

#[tauri::command(async)]
async fn prove_mint(value: String, nonce: String) -> Result<ProveResult, String> {
    let manifest = load_manifest().map_err(|e| e.to_string())?;
    let proof = prove_mint_blocking(manifest, value, nonce)
        .await
        .map_err(|e| e.to_string())?;
    Ok(ProveResult {
        encrypted_value: proof.encrypted_value,
        proof_bytes: proof.proof.len(),
        instance_bytes: proof.instance.len(),
        k: proof.k,
    })
}

async fn execute_mint_transaction(
    client: &SuiPtbClient,
    manifest: &MintManifest,
    proof: &MintProof,
) -> Result<MintTx> {
    let plan = ChunkPlan::new(proof.proof.clone(), Some(DEFAULT_CHUNK_SIZE))?;
    if plan.chunks.len() != 1 {
        return execute_mint_fallback(client, manifest, proof).await;
    }

    let mut ptb = ProgrammableTransactionBuilder::new();
    let store = client.object_input(&mut ptb, &manifest.store, true).await?;
    let params = client
        .object_input(&mut ptb, &manifest.params_object_id, false)
        .await?;
    let vk = client
        .object_input(&mut ptb, &manifest.vk_object_id, false)
        .await?;
    let proof_builder = add_new_proof_builder_call(&mut ptb, &manifest.verifier_api_package)?;
    add_append_chunk_call(
        &mut ptb,
        &manifest.verifier_api_package,
        proof_builder,
        plan.chunks[0].clone(),
    )?;
    let proof_digest = ptb.pure(plan.digest.to_vec())?;
    let kzg_variant = ptb.pure(kzg_variant_u8(proof.variant))?;
    let k_present = ptb.pure(true)?;
    let k = ptb.pure(proof.k)?;
    let encrypted_amount = U256::from_str(&proof.encrypted_value)
        .with_context(|| format!("invalid encrypted u256: {}", proof.encrypted_value))?;
    let encrypted_amount = ptb.pure(encrypted_amount)?;
    add_move_call(
        &mut ptb,
        &manifest.app_package,
        &manifest.module,
        &manifest.function,
        vec![
            store,
            params,
            vk,
            proof_builder,
            proof_digest,
            kzg_variant,
            k_present,
            k,
            encrypted_amount,
        ],
    )?;

    let response = client.execute_ptb(ptb.finish()).await?;
    Ok(MintTx {
        proof_builder: None,
        proof_digest: plan.digest.to_vec(),
        tx_digest: response.digest,
        tx_mode: "single_ptb".to_string(),
    })
}

async fn execute_mint_fallback(
    client: &SuiPtbClient,
    manifest: &MintManifest,
    proof: &MintProof,
) -> Result<MintTx> {
    let uploaded = client
        .upload_proof(&manifest.verifier_api_package, proof.proof.clone())
        .await?;
    let response = client
        .move_call(
            &manifest.app_package,
            &manifest.module,
            "mint",
            vec![
                json!(manifest.store.as_str()),
                json!(manifest.params_object_id.as_str()),
                json!(manifest.vk_object_id.as_str()),
                json!(uploaded.proof_builder.as_str()),
                json!(uploaded.proof_digest),
                json!(kzg_variant_u8(proof.variant)),
                json!(true),
                json!(proof.k),
                json!(proof.encrypted_value.as_str()),
            ],
        )
        .await?;
    Ok(MintTx {
        proof_builder: Some(uploaded.proof_builder),
        proof_digest: uploaded.proof_digest,
        tx_digest: response.digest,
        tx_mode: "multi_tx_fallback".to_string(),
    })
}

#[tauri::command(async)]
async fn mint(value: String, nonce: String) -> Result<MintResult, String> {
    let manifest = load_manifest().map_err(|e| e.to_string())?;
    let proof = prove_mint_blocking(manifest.clone(), value, nonce)
        .await
        .map_err(|e| e.to_string())?;
    let client = client_from_manifest(&manifest)
        .await
        .map_err(|e| e.to_string())?;
    let mint_tx = execute_mint_transaction(&client, &manifest, &proof)
        .await
        .map_err(|e| e.to_string())?;
    let balance = client
        .read_object_field(&manifest.store, "balance")
        .await
        .map_err(|e| e.to_string())?;

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

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            compute_encrypted,
            prove_mint,
            mint
        ])
        .run(tauri::generate_context!())
        .expect("error while running zkmove-mint gui");
}
