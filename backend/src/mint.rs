use anyhow::{anyhow, Context, Result};
use move_core_types::{parser::parse_transaction_argument, u256::U256};
use serde::Deserialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use sui_ptb_helper::{
    move_call, MoveTarget, ObjectAccess, ProgrammableTransactionBuilder, SuiPtbClient, SuiPtbConfig,
};
use sui_verifier_api::artifact::{
    digest32, ensure_artifact_size, ChunkPlan, Digest32, DEFAULT_CHUNK_SIZE,
    MAX_CIRCUIT_INFO_BYTES, MAX_PARAMS_BYTES, MAX_VK_BYTES,
};
use sui_verifier_api::artifact_ptb::ArtifactPtb;
use zkmove_cli::{
    api,
    common::{
        get_circuit_config_args_from_move_toml, get_entry_call_from_move_toml, load_package,
        read_params, KZGVariant,
    },
};

const SUI_MAX_TRANSACTION_BYTES: usize = 128 * 1024;
const DEFAULT_GAS_BUDGET_VALUE: &str = "1000000000";
const MODULE_ARTIFACT_BUILDER: &str = "artifact_builder";
const FUNC_PUBLISH_PARAMS_BUILDER: &str = "publish_params_builder";
const FUNC_PUBLISH_VK_BUILDER: &str = "publish_vk_builder";
const FUNC_PUBLISH_CIRCUIT_BUILDER: &str = "publish_circuit_info_builder";
const FUNC_APPEND_CHUNK: &str = "append_chunk";
const FUNC_FINALIZE_PARAMS_TO_SENDER: &str = "finalize_params_to_sender";
const FUNC_FINALIZE_VK_TO_SENDER: &str = "finalize_vk_to_sender";
const TYPE_SUFFIX_ARTIFACT_BUILDER: &str = "::artifact_builder::ArtifactBuilder";
const TYPE_SUFFIX_SERIALIZED_PARAMS: &str = "::serialized_params_store::SerializedParams";
const TYPE_SUFFIX_SERIALIZED_VK: &str = "::native_verifier::SerializedVK";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MintManifest {
    #[serde(default)]
    app_package: String,
    #[serde(default)]
    module: String,
    #[serde(default)]
    function: String,
    #[serde(default)]
    store: String,
    #[serde(default)]
    verifier_api_package: String,
    #[serde(default)]
    params_object_id: String,
    #[serde(default)]
    vk_object_id: String,
    #[serde(default = "default_gas_budget")]
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

fn default_gas_budget() -> String {
    DEFAULT_GAS_BUDGET_VALUE.to_string()
}

struct VerifierArtifacts {
    encrypted_value: String,
    k: u32,
    params: Vec<u8>,
    vk: Vec<u8>,
    circuit_info: Vec<u8>,
}

struct UploadedVerifierArtifacts {
    params_object_id: String,
    vk_object_id: String,
    params_digest: Vec<u8>,
    vk_digest: Vec<u8>,
    circuit_digest: Vec<u8>,
    tx_digest: String,
}

pub struct VerifierSetup {
    pub encrypted_value: String,
    pub k: u32,
    pub params_object_id: String,
    pub vk_object_id: String,
    pub verifier_api_package: String,
    pub tx_digest: String,
    pub manifest_path: String,
    pub params_bytes: usize,
    pub vk_bytes: usize,
    pub circuit_info_bytes: usize,
    pub params_digest: Vec<u8>,
    pub vk_digest: Vec<u8>,
    pub circuit_digest: Vec<u8>,
}

pub struct MintProof {
    pub encrypted_value: String,
    pub proof: Vec<u8>,
    pub instance: Vec<u8>,
    pub variant: KZGVariant,
    pub k: u32,
}

pub struct MintTx {
    pub proof_builder: Option<String>,
    pub proof_digest: Vec<u8>,
    pub tx_digest: String,
    pub tx_mode: String,
}

pub struct MintExecution {
    pub proof: MintProof,
    pub tx: MintTx,
    pub store_balance: String,
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
    if manifest.gas_budget.trim().is_empty() {
        manifest.gas_budget = default_gas_budget();
    }
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

fn read_manifest_json(path: &Path) -> Result<Value> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("failed to read manifest {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse manifest {}", path.display()))
}

fn write_setup_manifest(
    path: &Path,
    verifier_api_package: &str,
    params_object_id: &str,
    vk_object_id: &str,
    k: u32,
) -> Result<()> {
    let mut value = read_manifest_json(path)?;
    let root = value
        .as_object_mut()
        .ok_or_else(|| anyhow!("manifest root must be a JSON object"))?;
    root.insert(
        "verifierApiPackage".to_string(),
        Value::String(verifier_api_package.to_string()),
    );
    root.insert(
        "paramsObjectId".to_string(),
        Value::String(params_object_id.to_string()),
    );
    root.insert(
        "vkObjectId".to_string(),
        Value::String(vk_object_id.to_string()),
    );
    root.entry("gasBudget".to_string())
        .or_insert_with(|| Value::String(default_gas_budget()));

    let circuit = root
        .entry("circuit".to_string())
        .or_insert_with(|| Value::Object(Default::default()))
        .as_object_mut()
        .ok_or_else(|| anyhow!("manifest circuit must be a JSON object"))?;
    circuit.insert("k".to_string(), Value::Number(k.into()));

    let rendered = format!("{}\n", serde_json::to_string_pretty(&value)?);
    std::fs::write(path, rendered)
        .with_context(|| format!("failed to write manifest {}", path.display()))?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn setup_writeback_error(
    path: &Path,
    verifier_api_package: &str,
    params_object_id: &str,
    vk_object_id: &str,
    k: u32,
    tx_digest: &str,
    err: anyhow::Error,
) -> anyhow::Error {
    anyhow!(
        "verifier artifacts uploaded, but failed to write manifest {}: {err:#}\nmanual recovery: verifierApiPackage={}, paramsObjectId={}, vkObjectId={}, circuit.k={}, txDigest={}",
        path.display(),
        verifier_api_package,
        params_object_id,
        vk_object_id,
        k,
        tx_digest,
    )
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

fn build_verifier_artifacts_sync(
    manifest: &MintManifest,
    value: &str,
    nonce: &str,
) -> Result<VerifierArtifacts> {
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
    let variant = parse_kzg_variant(&manifest.circuit.kzg)?;
    let proof = api::prove::prove_with_witness(&ctx, &traces, variant)?;
    api::verify(&ctx, variant, &proof.proof, &proof.instance)?;

    let params = halo2_verifier::params::serialize_kzg_params(&ctx.params.verifier_params())
        .map_err(|err| anyhow!("serialize verifier params failed: {err}"))?;
    let vk = ctx.vk_bytes();
    let (circuit, _circuit_guard) = api::circuit::build_circuit_from_trace(
        &ctx.package,
        &traces,
        ctx.config.clone(),
        &ctx.pubs_indices,
    )?;
    let circuit_info =
        halo2_verifier::circuit::generate_serialized_circuit(&ctx.params, circuit.as_ref())
            .map_err(|err| anyhow!("generate serialized circuit info failed: {err:?}"))?;

    Ok(VerifierArtifacts {
        encrypted_value,
        k: ctx.k,
        params,
        vk,
        circuit_info,
    })
}

pub async fn prove_mint_blocking(
    manifest: MintManifest,
    value: String,
    nonce: String,
) -> Result<MintProof> {
    tauri::async_runtime::spawn_blocking(move || prove_mint_sync(&manifest, &value, &nonce))
        .await
        .map_err(|err| anyhow!("proof generation task failed: {err}"))?
}

async fn build_verifier_artifacts_blocking(
    manifest: MintManifest,
    value: String,
    nonce: String,
) -> Result<VerifierArtifacts> {
    tauri::async_runtime::spawn_blocking(move || {
        build_verifier_artifacts_sync(&manifest, &value, &nonce)
    })
    .await
    .map_err(|err| anyhow!("verifier artifact setup task failed: {err}"))?
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

fn artifact_target(verifier_api_package: &str, function: &str) -> Result<MoveTarget> {
    MoveTarget::new(verifier_api_package, MODULE_ARTIFACT_BUILDER, function)
}

async fn publish_artifact_builder(
    client: &SuiPtbClient,
    verifier_api_package: &str,
    function: &str,
    label: &str,
) -> Result<String> {
    let mut ptb = ProgrammableTransactionBuilder::new();
    move_call(
        &mut ptb,
        artifact_target(verifier_api_package, function)?,
        vec![],
    );
    let response = client
        .execute(ptb.finish())
        .await
        .with_context(|| format!("failed to create {label} artifact builder"))?;
    response
        .created_object_id(TYPE_SUFFIX_ARTIFACT_BUILDER)
        .with_context(|| format!("failed to parse {label} artifact builder object id"))
}

async fn append_artifact_chunks(
    client: &SuiPtbClient,
    verifier_api_package: &str,
    builder_id: &str,
    bytes: Vec<u8>,
    label: &str,
    max_bytes: usize,
) -> Result<Digest32> {
    ensure_artifact_size(label, bytes.len(), max_bytes)?;
    let plan = ChunkPlan::new(bytes, Some(DEFAULT_CHUNK_SIZE))
        .with_context(|| format!("failed to chunk {label} artifact"))?;
    let digest = plan.digest;

    for (idx, chunk) in plan.chunks.into_iter().enumerate() {
        let mut ptb = ProgrammableTransactionBuilder::new();
        let builder = client
            .input_object(&mut ptb, builder_id, ObjectAccess::Mutable)
            .await
            .with_context(|| format!("failed to load {label} builder {builder_id}"))?;
        let chunk = ptb
            .pure(chunk)
            .with_context(|| format!("failed to encode {label} chunk {}", idx + 1))?;
        move_call(
            &mut ptb,
            artifact_target(verifier_api_package, FUNC_APPEND_CHUNK)?,
            vec![builder, chunk],
        );
        client
            .execute(ptb.finish())
            .await
            .with_context(|| format!("failed to append {label} chunk {}", idx + 1))?;
    }

    Ok(digest)
}

async fn finalize_params_artifact(
    client: &SuiPtbClient,
    verifier_api_package: &str,
    builder_id: &str,
    digest: Digest32,
) -> Result<(String, String)> {
    let mut ptb = ProgrammableTransactionBuilder::new();
    let builder = client
        .input_object(&mut ptb, builder_id, ObjectAccess::Mutable)
        .await
        .with_context(|| format!("failed to load params builder {builder_id}"))?;
    let digest = ptb
        .pure(digest.to_vec())
        .context("failed to encode params digest")?;
    move_call(
        &mut ptb,
        artifact_target(verifier_api_package, FUNC_FINALIZE_PARAMS_TO_SENDER)?,
        vec![builder, digest],
    );
    let response = client
        .execute(ptb.finish())
        .await
        .context("failed to finalize params artifact")?;
    let object_id = response
        .created_object_id(TYPE_SUFFIX_SERIALIZED_PARAMS)
        .context("failed to parse SerializedParams object id")?;
    Ok((object_id, response.digest))
}

async fn finalize_vk_artifact(
    client: &SuiPtbClient,
    verifier_api_package: &str,
    vk_builder_id: &str,
    circuit_builder_id: &str,
    vk_digest: Digest32,
    circuit_digest: Digest32,
) -> Result<(String, String)> {
    let mut ptb = ProgrammableTransactionBuilder::new();
    let vk_builder = client
        .input_object(&mut ptb, vk_builder_id, ObjectAccess::Mutable)
        .await
        .with_context(|| format!("failed to load vk builder {vk_builder_id}"))?;
    let circuit_builder = client
        .input_object(&mut ptb, circuit_builder_id, ObjectAccess::Mutable)
        .await
        .with_context(|| format!("failed to load circuit builder {circuit_builder_id}"))?;
    let vk_digest = ptb
        .pure(vk_digest.to_vec())
        .context("failed to encode vk digest")?;
    let circuit_digest = ptb
        .pure(circuit_digest.to_vec())
        .context("failed to encode circuit digest")?;
    move_call(
        &mut ptb,
        artifact_target(verifier_api_package, FUNC_FINALIZE_VK_TO_SENDER)?,
        vec![vk_builder, circuit_builder, vk_digest, circuit_digest],
    );
    let response = client
        .execute(ptb.finish())
        .await
        .context("failed to finalize vk artifact")?;
    let object_id = response
        .created_object_id(TYPE_SUFFIX_SERIALIZED_VK)
        .context("failed to parse SerializedVK object id")?;
    Ok((object_id, response.digest))
}

async fn upload_verifier_artifacts_chunked(
    client: &SuiPtbClient,
    verifier_api_package: &str,
    params: Vec<u8>,
    vk: Vec<u8>,
    circuit: Vec<u8>,
) -> Result<UploadedVerifierArtifacts> {
    let params_digest = digest32(&params);
    let vk_digest = digest32(&vk);
    let circuit_digest = digest32(&circuit);

    let params_builder = publish_artifact_builder(
        client,
        verifier_api_package,
        FUNC_PUBLISH_PARAMS_BUILDER,
        "params",
    )
    .await?;
    let vk_builder =
        publish_artifact_builder(client, verifier_api_package, FUNC_PUBLISH_VK_BUILDER, "vk")
            .await?;
    let circuit_builder = publish_artifact_builder(
        client,
        verifier_api_package,
        FUNC_PUBLISH_CIRCUIT_BUILDER,
        "circuit",
    )
    .await?;

    let appended_params_digest = append_artifact_chunks(
        client,
        verifier_api_package,
        &params_builder,
        params,
        "params",
        MAX_PARAMS_BYTES,
    )
    .await?;
    let appended_vk_digest = append_artifact_chunks(
        client,
        verifier_api_package,
        &vk_builder,
        vk,
        "vk",
        MAX_VK_BYTES,
    )
    .await?;
    let appended_circuit_digest = append_artifact_chunks(
        client,
        verifier_api_package,
        &circuit_builder,
        circuit,
        "circuit info",
        MAX_CIRCUIT_INFO_BYTES,
    )
    .await?;

    if appended_params_digest != params_digest
        || appended_vk_digest != vk_digest
        || appended_circuit_digest != circuit_digest
    {
        return Err(anyhow!(
            "internal digest mismatch while chunking verifier artifacts"
        ));
    }

    let (params_object_id, _params_tx_digest) =
        finalize_params_artifact(client, verifier_api_package, &params_builder, params_digest)
            .await?;
    let (vk_object_id, tx_digest) = finalize_vk_artifact(
        client,
        verifier_api_package,
        &vk_builder,
        &circuit_builder,
        vk_digest,
        circuit_digest,
    )
    .await?;

    Ok(UploadedVerifierArtifacts {
        params_object_id,
        vk_object_id,
        params_digest: params_digest.to_vec(),
        vk_digest: vk_digest.to_vec(),
        circuit_digest: circuit_digest.to_vec(),
        tx_digest,
    })
}

fn validate_mint_manifest_ready(manifest: &MintManifest) -> Result<()> {
    let mut missing = Vec::new();
    for (field, value) in [
        ("appPackage", &manifest.app_package),
        ("module", &manifest.module),
        ("function", &manifest.function),
        ("store", &manifest.store),
        ("verifierApiPackage", &manifest.verifier_api_package),
        ("paramsObjectId", &manifest.params_object_id),
        ("vkObjectId", &manifest.vk_object_id),
        ("gasBudget", &manifest.gas_budget),
    ] {
        if value.trim().is_empty() {
            missing.push(field);
        }
    }
    if manifest.circuit.k.is_none() {
        missing.push("circuit.k");
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(
            "manifest is not ready for mint; missing {}. Run GUI Setup Verifier Artifacts first, then make sure appPackage/store come from the current localnet run.",
            missing.join(", ")
        ))
    }
}

pub async fn setup_verifier_artifacts(
    manifest_path: PathBuf,
    value: String,
    nonce: String,
    verifier_api_package: String,
) -> Result<VerifierSetup> {
    let mut manifest = load_manifest_from_path(manifest_path.clone())?;
    let verifier_api_package = if verifier_api_package.trim().is_empty() {
        manifest.verifier_api_package.clone()
    } else {
        verifier_api_package.trim().to_string()
    };
    if verifier_api_package.trim().is_empty() {
        return Err(anyhow!(
            "verifierApiPackage is required before uploading verifier artifacts"
        ));
    }
    manifest.verifier_api_package = verifier_api_package.clone();

    let artifacts = build_verifier_artifacts_blocking(manifest.clone(), value, nonce).await?;
    let params_bytes = artifacts.params.len();
    let vk_bytes = artifacts.vk.len();
    let circuit_info_bytes = artifacts.circuit_info.len();
    let k = artifacts.k;
    let encrypted_value = artifacts.encrypted_value.clone();

    let client = client_from_manifest(&manifest).await?;
    let receipt = upload_verifier_artifacts_chunked(
        &client,
        &verifier_api_package,
        artifacts.params,
        artifacts.vk,
        artifacts.circuit_info,
    )
    .await
    .with_context(|| {
        format!(
            "failed to upload verifier artifacts with verifierApiPackage {}",
            verifier_api_package
        )
    })?;

    if let Err(err) = write_setup_manifest(
        &manifest_path,
        &verifier_api_package,
        &receipt.params_object_id,
        &receipt.vk_object_id,
        k,
    ) {
        return Err(setup_writeback_error(
            &manifest_path,
            &verifier_api_package,
            &receipt.params_object_id,
            &receipt.vk_object_id,
            k,
            &receipt.tx_digest,
            err,
        ));
    }

    Ok(VerifierSetup {
        encrypted_value,
        k,
        params_object_id: receipt.params_object_id,
        vk_object_id: receipt.vk_object_id,
        verifier_api_package,
        tx_digest: receipt.tx_digest,
        manifest_path: manifest_path.display().to_string(),
        params_bytes,
        vk_bytes,
        circuit_info_bytes,
        params_digest: receipt.params_digest,
        vk_digest: receipt.vk_digest,
        circuit_digest: receipt.circuit_digest,
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

    let response = client.execute(ptb.finish()).await.map_err(|err| {
        let err_text = err.to_string();
        if err_text.contains("TypeMismatch") {
            anyhow!(
                "{err_text}\nmanifest object type mismatch: ensure store was created by appPackage {}, and paramsObjectId/vkObjectId were created by verifierApiPackage {} in the same localnet run",
                manifest.app_package,
                manifest.verifier_api_package
            )
        } else {
            err
        }
    })?;
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

pub async fn mint_value(
    manifest: MintManifest,
    value: String,
    nonce: String,
) -> Result<MintExecution> {
    validate_mint_manifest_ready(&manifest)?;
    let proof = prove_mint_blocking(manifest.clone(), value, nonce).await?;
    let client = client_from_manifest(&manifest).await?;
    let mint_tx =
        execute_mint_transaction(&client, &manifest, &proof, &proof.encrypted_value).await?;
    let balance = read_store_balance(&client, &manifest).await?;

    Ok(MintExecution {
        proof,
        tx: mint_tx,
        store_balance: balance,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_manifest_path(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("{name}-{nanos}.json"))
    }

    #[test]
    fn write_setup_manifest_updates_artifact_fields() {
        let path = temp_manifest_path("zkmove-mint-setup-manifest");
        std::fs::write(
            &path,
            r#"{
  "chain": "sui-localnet",
  "appPackage": "",
  "module": "mint_min",
  "function": "mint_from_builder",
  "store": "",
  "verifierApiPackage": "0xold",
  "paramsObjectId": "",
  "vkObjectId": "",
  "gasBudget": "1000000000",
  "circuit": {
    "packagePath": "../off-chain",
    "entryFunction": "encrypt",
    "circuitName": "encrypt",
    "pubsIndices": [1],
    "kzg": "gwc",
    "k": null,
    "srsPath": "../params.srs"
  }
}"#,
        )
        .unwrap();

        write_setup_manifest(&path, "0xapi", "0xparams", "0xvk", 12).unwrap();
        let value = read_manifest_json(&path).unwrap();

        assert_eq!(value["verifierApiPackage"], "0xapi");
        assert_eq!(value["paramsObjectId"], "0xparams");
        assert_eq!(value["vkObjectId"], "0xvk");
        assert_eq!(value["circuit"]["k"], 12);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn setup_writeback_error_contains_manual_recovery_fields() {
        let err = setup_writeback_error(
            Path::new("/tmp/missing/manifest.json"),
            "0xapi",
            "0xparams",
            "0xvk",
            12,
            "8abc",
            anyhow!("permission denied"),
        );
        let text = err.to_string();

        assert!(text.contains("verifier artifacts uploaded"));
        assert!(text.contains("verifierApiPackage=0xapi"));
        assert!(text.contains("paramsObjectId=0xparams"));
        assert!(text.contains("vkObjectId=0xvk"));
        assert!(text.contains("circuit.k=12"));
        assert!(text.contains("txDigest=8abc"));
    }
}
