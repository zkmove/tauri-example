# zkMove Confidential Mint MVP

这个仓库是 `zkMove Confidential Mint` 的 MVP 应用。它包含一个 Tauri GUI backend、一个最小 Sui Move package `mint-min`，以及用于 localnet 验收的 single PTB harness。

MVP 验证的核心语义是：

1. 用户本地知道 `(value, nonce)`。
2. `encrypted_amount = poseidon(value, nonce)`。
3. proof 将 `encrypted_amount` 作为 public input 暴露，也就是 `pubs_indices = [1]`。
4. 链上 `mint_min::mint_from_builder` 根据 `encrypted_amount` 重建 public inputs。
5. proof 通过 `artifact_builder::verify_proof_from_builder` 后才 mint。
6. 用 A amount 的 proof 去 mint B amount 必须失败。

## 1. 准备环境

下面的命令用环境变量表示本地路径。默认假设几个 repo 放在同一个父目录下；如果你的目录不同，只需要改这些变量。

```bash
export WORK_ROOT="$HOME/work/project"
export APP_REPO="$WORK_ROOT/tauri-example"
export ZKMOVE_REPO="$WORK_ROOT/zkmove"
export ZKMOVE_VM_REPO="$WORK_ROOT/zkmove-vm"
export HALO2_REPO="$WORK_ROOT/halo2-verifier.move"
export SUI_REPO="$WORK_ROOT/zkmove_sui"
```

当前 MVP 默认依赖 sibling repo 布局：

```bash
$APP_REPO
$ZKMOVE_REPO
$ZKMOVE_VM_REPO
$HALO2_REPO
$SUI_REPO
```

确认关键文件存在：

```bash
cd "$APP_REPO"
test -f backend/Cargo.toml
test -f mint-min.manifest.json
test -f mint-min/Move.toml
test -x "$SUI_REPO/target/debug/sui"
```

如果 `sui` binary 不存在，先在 `$SUI_REPO` 构建对应分支的 Sui binary。

## 2. 配置 localnet

GUI 和 harness 默认读取：

```bash
$APP_REPO/mint-min.manifest.json
```

`mint-min.manifest.json` 必须匹配当前 localnet，尤其是这些字段：

```json
{
  "appPackage": "0x...",
  "store": "0x...",
  "verifierApiPackage": "0x...",
  "paramsObjectId": "0x...",
  "vkObjectId": "0x..."
}
```

运行前确认 localnet 已启动，并设置本地账户：

```bash
export SUI_RPC_URL=http://127.0.0.1:9000
export SUI_KEYSTORE_PATH=/path/to/sui.keystore
export SUI_SENDER=0x...
```

可选覆盖项：

```bash
export ZKMOVE_MINT_MANIFEST=/path/to/mint-min.manifest.json
export ZKMOVE_MINT_PACKAGE_PATH=/path/to/off-chain
export ZKMOVE_MINT_SRS_PATH=/path/to/kzg_bn254_12.srs
```

`SUI_SENDER` 必须拥有足够 gas。重新启动或重新发布 localnet 后，需要刷新 `mint-min.manifest.json` 中的 package/object ids。

## 3. 构建 Move package

构建 `mint-min`：

```bash
cd "$APP_REPO/mint-min"
"$SUI_REPO/target/debug/sui" move build --build-env testnet --silence-warnings
```

`mint-min` 的 PTB 主路径调用 composable function：

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

默认参数：

- `pubs_indices = [1]`
- `kzg_variant = 0` (`GWC`)
- `k_present = true`
- `k` 以 `setup_with_witness` 返回的 `ctx.k` 为准；manifest 中的 `k` 只做一致性断言
- proof upload chunk size 默认 `15360` bytes

## 4. 运行 GUI

GUI backend 不调用 `sdk/zkmove-mint.sh`、`zkmove` CLI 或 `upload_sui_*.sh`。当前路径是：

```text
Tauri UI
  -> zkmove_cli::api::{poseidon_hash, generate_witness, setup_with_witness, prove_with_witness, verify}
  -> sui_verifier_api::artifact_ptb::ArtifactPtb
  -> single PTB: ArtifactPtb::stage_proof
               + mint_min::mint_from_builder
  -> sui_ptb_helper::SuiPtbClient::read_object_field(balance)
```

启动 GUI：

```bash
cd "$APP_REPO/backend"
cargo run
```

在窗口中输入 `amount` 和 `nonce` 后点击 `Mint`。本地会生成 witness/proof，先本地 verify，再用 single PTB 提交链上 mint。

成功结果应包含：

```text
tx mode = single_ptb
tx <digest> -> success
```

## 5. 运行 localnet PTB harness

PTB 主验收使用 harness，不使用旧的 `upload_sui_proof.sh` 多交易路径：

```bash
cd "$APP_REPO"
./mint-min/run-localnet-ptb-harness.sh --value 6 --nonce 0 --negative
```

预期输出必须包含：

```text
positive txMode=single_ptb
negative txMode=single_ptb status=aborted
negative balanceUnchanged=true
```

这说明：

- positive mint 成功，并且 `Store.balance == encrypted_amount`。
- negative case 复用同一 proof，但传入 `encrypted_amount + 1`，链上 abort。
- abort 后 balance 不变。

## 6. 验收检查

推荐按下面顺序检查：

```bash
cd "$HALO2_REPO"
cargo test -p sui-verifier-api
cargo test -p sui-ptb-helper

cd "$ZKMOVE_VM_REPO"
cargo check -p zkmove-cli

cd "$APP_REPO"
cargo check --manifest-path backend/Cargo.toml
./mint-min/run-localnet-ptb-harness.sh --value 6 --nonce 0 --negative
```

最终验收标准：

1. `sui-verifier-api` tests 通过。
2. `sui-ptb-helper` tests 通过。
3. `zkmove-cli` check 通过。
4. `backend` check 通过。
5. positive mint 输出 `txMode=single_ptb`，且 `Store.balance == encrypted_amount`。
6. negative mint 输出 `txMode=single_ptb status=aborted`，且 balance 不变。

## 7. 常见问题

`localnet_ptb_harness` 找不到 manifest：

- 确认 `ZKMOVE_MINT_MANIFEST` 是否指向存在的 JSON 文件。
- 未设置时默认使用当前 repo 下的 `mint-min.manifest.json`。

链上 object 找不到或 type mismatch：

- 重新检查 `appPackage`、`store`、`verifierApiPackage`、`paramsObjectId`、`vkObjectId` 是否来自同一个 localnet。
- localnet 重启或重新 publish 后，旧 object id 通常不可继续使用。

交易签名或 gas 报错：

- 确认 `SUI_KEYSTORE_PATH` 指向正确 keystore。
- 确认 `SUI_SENDER` 是 keystore 中的地址。
- 确认 `SUI_SENDER` 在当前 localnet 有 gas。

proof 过大：

- 当前 MVP 只接受 single PTB 路径。
- proof 超过当前 single PTB 限制时会 fail closed，不会降级到非原子多交易路径。

## Known Limitations

- strong binding 依赖 `zkmove-vm` 的 public-input row mapping 修复；当前要求 checkout 停在 `codex/fix-public-input-row-mapping`，至少包含 `419b000 fix public input row mapping`。该修复未合入 main 前，不能声称“任意同事拉 main 即可复现强绑定验收”。
- Tauri backend 依赖 `zkmove-vm/cli/src/api/setup.rs` 和 `mod.rs` 中 `setup/setup_with_witness` 的 public export 改动；这些改动必须提交或固定分支。
- empty public inputs 只能作为 smoke path，不能作为最终 MVP 验收。
- `sui-ptb-helper` 与 Tauri backend 必须完成真实 `cargo check/test` 后，才能把状态从“代码已调整”升级为“编译验证通过”。
- setup 闭环仍未完全落地：`sui-verifier-api::artifact_ptb::ArtifactUploader::upload_verifier_artifacts` 已提供单笔 PTB 上传能力，但 demo backend 还没有生成 `params/vk/circuit_info` 并写回 manifest 的 setup command；重新 localnet publish 后，manifest 中的 package/object ids 仍需刷新。
