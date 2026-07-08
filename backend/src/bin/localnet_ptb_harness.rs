use anyhow::{anyhow, ensure, Context, Result};
use move_core_types::u256::U256;
use std::env;
use std::path::PathBuf;
use std::str::FromStr;
use zkmove_mint_gui::mint;

#[derive(Debug)]
struct Args {
    manifest: Option<PathBuf>,
    value: String,
    nonce: String,
    negative: bool,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = parse_args()?;
    let manifest = match args.manifest {
        Some(path) => mint::load_manifest_from_path(path)?,
        None => mint::load_manifest()?,
    };

    let proof = mint::prove_mint_sync(&manifest, &args.value, &args.nonce)?;
    let client = tauri::async_runtime::block_on(mint::client_from_manifest(&manifest))?;

    let positive = tauri::async_runtime::block_on(mint::execute_mint_transaction(
        &client,
        &manifest,
        &proof,
        &proof.encrypted_value,
    ))?;
    ensure!(
        positive.tx_mode == "single_ptb",
        "positive mint used unexpected tx mode: {}",
        positive.tx_mode
    );
    let balance = tauri::async_runtime::block_on(mint::read_store_balance(&client, &manifest))?;
    ensure!(
        balance == proof.encrypted_value,
        "balance after positive mint is {balance}, expected {}",
        proof.encrypted_value
    );
    println!(
        "positive txMode={} digest={} balance={}",
        positive.tx_mode, positive.tx_digest, balance
    );

    if args.negative {
        let tampered_encrypted = increment_u256_string(&proof.encrypted_value)?;
        match tauri::async_runtime::block_on(mint::execute_mint_transaction(
            &client,
            &manifest,
            &proof,
            &tampered_encrypted,
        )) {
            Ok(tx) => {
                return Err(anyhow!(
                    "negative mint unexpectedly succeeded with txMode={} digest={}",
                    tx.tx_mode,
                    tx.tx_digest
                ));
            }
            Err(err) => {
                let err_text = format!("{err:#}");
                ensure!(
                    err_text.contains("EInvalidProof")
                        || err_text.contains("MoveAbort")
                        || err_text.contains("abort_code: 1"),
                    "negative mint failed with unexpected error: {err_text}"
                );
                println!(
                    "negative txMode=single_ptb status=aborted reason={}",
                    summarize_expected_abort(&err_text)
                );
            }
        }

        let balance_after_negative =
            tauri::async_runtime::block_on(mint::read_store_balance(&client, &manifest))?;
        ensure!(
            balance_after_negative == balance,
            "balance changed after negative mint: before {balance}, after {balance_after_negative}"
        );
        println!(
            "negative balanceUnchanged=true balance={}",
            balance_after_negative
        );
    }

    Ok(())
}

fn summarize_expected_abort(err_text: &str) -> &'static str {
    if err_text.contains("EInvalidProof")
        || err_text.contains("abort_code: 1")
        || err_text.contains("MoveAbort") && err_text.contains("mint_min")
    {
        "mint_min::EInvalidProof"
    } else {
        "expected negative-case abort"
    }
}

fn parse_args() -> Result<Args> {
    let mut manifest = None;
    let mut value = None;
    let mut nonce = None;
    let mut negative = false;
    let mut args = env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--manifest" => {
                let path = args
                    .next()
                    .ok_or_else(|| anyhow!("--manifest requires a path"))?;
                manifest = Some(PathBuf::from(path));
            }
            "--value" => {
                value = Some(
                    args.next()
                        .ok_or_else(|| anyhow!("--value requires a decimal u128"))?,
                );
            }
            "--nonce" => {
                nonce = Some(
                    args.next()
                        .ok_or_else(|| anyhow!("--nonce requires a decimal u128"))?,
                );
            }
            "--negative" => negative = true,
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            other => return Err(anyhow!("unknown argument: {other}")),
        }
    }

    Ok(Args {
        manifest,
        value: value.unwrap_or_else(|| "6".to_string()),
        nonce: nonce.unwrap_or_else(|| "0".to_string()),
        negative,
    })
}

fn increment_u256_string(value: &str) -> Result<String> {
    let value =
        U256::from_str(value).with_context(|| format!("invalid encrypted u256: {value}"))?;
    value
        .checked_add(U256::from(1u8))
        .map(|value| value.to_string())
        .ok_or_else(|| anyhow!("encrypted u256 overflow when adding 1"))
}

fn print_usage() {
    eprintln!(
        "Usage: localnet-ptb-harness [--manifest PATH] [--value U128] [--nonce U128] [--negative]"
    );
}
