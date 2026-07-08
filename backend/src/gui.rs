use crate::mint as mint_api;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProveResult {
    pub encrypted_value: String,
    pub proof_bytes: usize,
    pub instance_bytes: usize,
    pub k: u32,
}

impl From<mint_api::MintProof> for ProveResult {
    fn from(proof: mint_api::MintProof) -> Self {
        Self {
            encrypted_value: proof.encrypted_value,
            proof_bytes: proof.proof.len(),
            instance_bytes: proof.instance.len(),
            k: proof.k,
        }
    }
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

impl From<mint_api::MintExecution> for MintResult {
    fn from(execution: mint_api::MintExecution) -> Self {
        Self {
            encrypted_value: execution.proof.encrypted_value,
            proof_bytes: execution.proof.proof.len(),
            instance_bytes: execution.proof.instance.len(),
            k: execution.proof.k,
            proof_builder: execution.tx.proof_builder,
            proof_digest: execution.tx.proof_digest,
            tx_digest: execution.tx.tx_digest,
            tx_mode: execution.tx.tx_mode,
            store_balance: execution.store_balance,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupVerifierRequest {
    pub value: String,
    pub nonce: String,
    pub verifier_api_package: String,
    pub manifest_path: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupResult {
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

impl From<mint_api::VerifierSetup> for SetupResult {
    fn from(setup: mint_api::VerifierSetup) -> Self {
        Self {
            encrypted_value: setup.encrypted_value,
            k: setup.k,
            params_object_id: setup.params_object_id,
            vk_object_id: setup.vk_object_id,
            verifier_api_package: setup.verifier_api_package,
            tx_digest: setup.tx_digest,
            manifest_path: setup.manifest_path,
            params_bytes: setup.params_bytes,
            vk_bytes: setup.vk_bytes,
            circuit_info_bytes: setup.circuit_info_bytes,
            params_digest: setup.params_digest,
            vk_digest: setup.vk_digest,
            circuit_digest: setup.circuit_digest,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestStatus {
    pub manifest_path: String,
    pub ready: bool,
    pub missing_fields: Vec<String>,
}

fn manifest_path_from_option(path: Option<String>) -> PathBuf {
    path.as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(mint_api::manifest_path)
}

fn error_text(err: anyhow::Error) -> String {
    format!("{err:#}")
}

fn load_manifest_from_option(path: Option<String>) -> Result<mint_api::MintManifest> {
    mint_api::load_manifest_from_path(manifest_path_from_option(path))
}

fn read_manifest_json(path: &Path) -> Result<Value> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("failed to read manifest {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse manifest {}", path.display()))
}

fn non_empty_json_string(value: &Value, field: &str) -> bool {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

fn missing_manifest_fields(value: &Value) -> Vec<String> {
    let mut missing = Vec::new();
    for field in [
        "appPackage",
        "module",
        "function",
        "store",
        "verifierApiPackage",
        "paramsObjectId",
        "vkObjectId",
        "gasBudget",
    ] {
        if !non_empty_json_string(value, field) {
            missing.push(field.to_string());
        }
    }
    if !value
        .get("circuit")
        .and_then(|circuit| circuit.get("k"))
        .map(|k| k.is_number())
        .unwrap_or(false)
    {
        missing.push("circuit.k".to_string());
    }
    missing
}

fn manifest_status_value(manifest_path: Option<String>) -> Result<ManifestStatus> {
    let path = manifest_path_from_option(manifest_path);
    let value = read_manifest_json(&path)?;
    let missing_fields = missing_manifest_fields(&value);
    Ok(ManifestStatus {
        manifest_path: path.display().to_string(),
        ready: missing_fields.is_empty(),
        missing_fields,
    })
}

#[tauri::command(async)]
fn compute_encrypted(value: String, nonce: String) -> Result<String, String> {
    mint_api::compute_encrypted_value(value, nonce).map_err(error_text)
}

#[tauri::command(async)]
async fn prove_mint(
    value: String,
    nonce: String,
    manifest_path: Option<String>,
) -> Result<ProveResult, String> {
    let manifest = load_manifest_from_option(manifest_path).map_err(error_text)?;
    mint_api::prove_mint_blocking(manifest, value, nonce)
        .await
        .map(ProveResult::from)
        .map_err(error_text)
}

#[tauri::command(async)]
fn manifest_status(manifest_path: Option<String>) -> Result<ManifestStatus, String> {
    manifest_status_value(manifest_path).map_err(error_text)
}

#[tauri::command(async)]
async fn setup_verifier_artifacts(request: SetupVerifierRequest) -> Result<SetupResult, String> {
    let manifest_path = manifest_path_from_option(request.manifest_path);
    mint_api::setup_verifier_artifacts(
        manifest_path,
        request.value,
        request.nonce,
        request.verifier_api_package,
    )
    .await
    .map(SetupResult::from)
    .map_err(error_text)
}

#[tauri::command(async)]
async fn mint(
    value: String,
    nonce: String,
    manifest_path: Option<String>,
) -> Result<MintResult, String> {
    let manifest = load_manifest_from_option(manifest_path).map_err(error_text)?;
    mint_api::mint_value(manifest, value, nonce)
        .await
        .map(MintResult::from)
        .map_err(error_text)
}

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            compute_encrypted,
            prove_mint,
            manifest_status,
            setup_verifier_artifacts,
            mint
        ])
        .run(tauri::generate_context!())
        .expect("error while running zkmove-mint gui");
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
    fn manifest_path_option_trims_explicit_path() {
        assert_eq!(
            manifest_path_from_option(Some("  /tmp/mint-min.manifest.json  ".to_string())),
            PathBuf::from("/tmp/mint-min.manifest.json")
        );
    }

    #[test]
    fn manifest_status_uses_explicit_manifest_path() {
        let path = temp_manifest_path("zkmove-mint-status-manifest");
        std::fs::write(
            &path,
            r#"{
  "appPackage": "0xapp",
  "module": "mint_min",
  "function": "mint_from_builder",
  "store": "0xstore",
  "verifierApiPackage": "0xapi",
  "paramsObjectId": "0xparams",
  "vkObjectId": "0xvk",
  "gasBudget": "1000000000",
  "circuit": {
    "packagePath": "../off-chain",
    "entryFunction": "encrypt",
    "circuitName": "encrypt",
    "pubsIndices": [1],
    "kzg": "gwc",
    "k": 12,
    "srsPath": "../params.srs"
  }
}"#,
        )
        .unwrap();

        let status = manifest_status_value(Some(path.display().to_string())).unwrap();

        assert!(status.ready);
        assert_eq!(status.manifest_path, path.display().to_string());
        assert!(status.missing_fields.is_empty());

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn manifest_status_reports_missing_fields() {
        let value = serde_json::json!({
            "appPackage": "",
            "module": "mint_min",
            "function": "mint_from_builder",
            "store": "",
            "verifierApiPackage": "0xapi",
            "paramsObjectId": "0xparams",
            "vkObjectId": "0xvk",
            "gasBudget": "1000000000",
            "circuit": {
                "k": null
            }
        });

        let missing = missing_manifest_fields(&value);

        assert!(missing.contains(&"appPackage".to_string()));
        assert!(missing.contains(&"store".to_string()));
        assert!(missing.contains(&"circuit.k".to_string()));
        assert!(!missing.contains(&"paramsObjectId".to_string()));
        assert!(!missing.contains(&"vkObjectId".to_string()));
    }
}
