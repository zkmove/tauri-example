#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use zkmove_mint_gui::mint_backend::{self, MintResult, ProveResult};

#[tauri::command(async)]
fn compute_encrypted(value: String, nonce: String) -> Result<String, String> {
    mint_backend::compute_encrypted_value(value, nonce).map_err(|e| e.to_string())
}

#[tauri::command(async)]
async fn prove_mint(value: String, nonce: String) -> Result<ProveResult, String> {
    mint_backend::prove_mint_value(value, nonce)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command(async)]
async fn mint(value: String, nonce: String) -> Result<MintResult, String> {
    mint_backend::mint_value(value, nonce)
        .await
        .map_err(|e| e.to_string())
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
