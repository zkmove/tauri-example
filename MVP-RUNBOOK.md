# MVP Runbook：在 Sui localnet 上验证 confidential-asset `mint`

> 当前版本：2026-07-02。`tauri-example` 是 GUI 与 `mint-min` 的新主项目。

## 0. 本文验证什么

这个 MVP 验证的是：

1. 用户知道 `(value, nonce)`。
2. `encrypted_amount = poseidon(value, nonce)`。
3. proof 将 `encrypted_amount` 作为 public input 暴露出来，也就是 `pubs_indices = [1]`。
4. `mint_min::mint` 在链上根据 `encrypted_amount` 重建 public inputs。
5. proof 通过 `artifact_builder::verify_proof_from_builder` 后才 mint。
6. 用 A amount 的 proof mint B amount 必须失败。

主项目路径：

```bash
/Users/ssyuan/work/project/tauri-example
```

依赖 repo 默认按 sibling 布局读取：

```bash
/Users/ssyuan/work/project/zkmove
/Users/ssyuan/work/project/zkmove-vm
/Users/ssyuan/work/project/halo2-verifier.move
/Users/ssyuan/work/project/zkmove_sui
```

## 1. GUI 主路径

GUI 后端不再调用 `sdk/zkmove-mint.sh`、`zkmove` CLI 或 `upload_sui_*.sh`。当前路径是：

```text
Tauri UI
  -> zkmove_cli::api::{poseidon_hash, generate_witness, setup_with_witness, prove_with_witness, verify}
  -> sui_ptb_helper::{ProgrammableTransactionBuilder, artifact builder helpers}
  -> single PTB: artifact_builder::new_proof_builder
               + artifact_builder::append_chunk
               + mint_min::mint_from_builder
  -> sui_ptb_helper::SuiPtbClient::read_object_field(balance)
```

`sui-ptb-helper` 只提供通用 PTB 执行、object/pure argument、chunk/artifact builder helper；`mint` 的业务编排在 `tauri-example` backend 中完成。proof 超过单 chunk 时才降级到 `multi_tx_fallback`，该路径仅保留兼容性，不作为 MVP 主验收路径。

运行配置来自：

```bash
/Users/ssyuan/work/project/tauri-example/mint-min.manifest.json
```

可覆盖的环境变量：

```bash
export ZKMOVE_MINT_MANIFEST=/path/to/mint-min.manifest.json
export SUI_RPC_URL=http://127.0.0.1:9000
export SUI_KEYSTORE_PATH=/path/to/sui.keystore
export SUI_SENDER=0x...
export ZKMOVE_MINT_PACKAGE_PATH=/path/to/off-chain
export ZKMOVE_MINT_SRS_PATH=/path/to/kzg_bn254_12.srs
```

启动 GUI：

```bash
cd /Users/ssyuan/work/project/tauri-example/src-tauri
cargo run
```

## 2. 合约与参数

PTB 主路径调用 demo 包里的 composable function：

```move
public fun mint_from_builder(
    store: &mut Store,
    params: &SerializedParams,
    vk: &SerializedVK,
    proof_builder: ArtifactBuilder,
    expected_proof_digest: vector<u8>,
    kzg_variant: u8,
    k_present: bool,
    k: u32,
    encrypted_amount: u256,
)
```

兼容脚本仍可调用 `entry fun mint`，参数顺序相同：

```move
entry fun mint(
    store: &mut Store,
    params: &SerializedParams,
    vk: &SerializedVK,
    proof_builder: ArtifactBuilder,
    expected_proof_digest: vector<u8>,
    kzg_variant: u8,
    k_present: bool,
    k: u32,
    encrypted_amount: u256,
)
```

默认参数：

- `pubs_indices = [1]`
- `kzg_variant = 0` (`GWC`)
- `k_present = true`
- `k` 以 `setup_with_witness` 返回的 `ctx.k` 为准；manifest 中的 `k` 只做一致性断言
- proof upload chunk size 默认 `15360` bytes

构建 `mint-min`：

```bash
cd /Users/ssyuan/work/project/tauri-example/mint-min
/Users/ssyuan/work/project/zkmove_sui/target/debug/sui move build --build-env testnet --silence-warnings
```

## 3. Localnet E2E 兼容脚本

兼容脚本仍保留，用于验证完整 localnet 发布与链上路径：

```bash
RUN_ID=mint-min-tauri-example \
RPC_PORT=9011 \
FAUCET_PORT=9134 \
/Users/ssyuan/work/project/tauri-example/mint-min/run-localnet-e2e.sh 6 0
```

`mint-min` Move package 的 source of truth 是：

```bash
/Users/ssyuan/work/project/tauri-example/mint-min
```

当前兼容脚本仍复用 `zkmove-vm/examples/confidential-asset/mint-min/run-localnet-e2e.sh` 的编排逻辑，但默认：

- `MINT_PACKAGE=/Users/ssyuan/work/project/tauri-example/mint-min`
- `OFFCHAIN_PACKAGE=/Users/ssyuan/work/project/zkmove/examples/confidential-asset/off-chain`
- `RUN_DIR=/Users/ssyuan/work/project/tauri-example/.zkmove/runs/$RUN_ID`

## 4. 验收标准

必须通过：

1. `cargo test -p sui-verifier-api`
2. `cargo test -p sui-ptb-helper`
3. `cargo check -p zkmove-cli`
4. `cargo check --manifest-path /Users/ssyuan/work/project/tauri-example/src-tauri/Cargo.toml`
5. localnet positive mint 成功，`Store.balance == encrypted_amount`
6. `encrypted_amount + 1` negative mint 以 `EInvalidProof = 1` abort，balance 不变

阻断级限制：

- strong binding 依赖 `zkmove-vm` 的 public-input row mapping 修复；当前要求 checkout 停在 `codex/fix-public-input-row-mapping`，至少包含 `419b000 fix public input row mapping`。该修复未合入 main 前，不能声称“任意同事拉 main 即可复现强绑定验收”。
- Tauri backend 依赖 `zkmove-vm/cli/src/api/setup.rs` 和 `mod.rs` 中 `setup/setup_with_witness` 的 public export 改动；这些改动必须提交或固定分支。
- empty public inputs 只能作为 smoke path，不能作为最终 MVP 验收。
- `sui-ptb-helper` 与 Tauri backend 必须完成真实 `cargo check/test` 后，才能把状态从“代码已调整”升级为“编译验证通过”。
- setup 闭环仍未完全落地：`sui-ptb-helper::upload_verifier_artifacts` 已提供上传能力，但 demo backend 还没有生成 `params/vk/circuit_info` 并写回 manifest 的 setup command；重新 localnet publish 后，manifest 中的 package/object ids 仍需刷新。
