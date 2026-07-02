# confidential-asset MVP 封装调整实施总结

> 日期：2026-07-02
> 目标：按团队讨论结论重整 Sui mint MVP 的 repo 边界，使终端用户最终能通过 GUI/最小路径完整跑通 token mint。

## 1. 总体状态

Review 后的修正状态：

- `sui-ptb-helper` 不再承诺任何 `mint` 业务 API；它只提供通用 PTB 执行、object input、artifact builder command helper、chunk plan。
- `tauri-example` backend 承接 demo-specific mint 编排，主路径使用单笔 PTB：`new_proof_builder -> append_chunk -> mint_from_builder`。
- `mint-min` 保留 `entry fun mint` 兼容脚本，同时新增 `public fun mint_from_builder` 供 PTB 组合调用。
- proof 超过单 chunk 时才走 `multi_tx_fallback`，该路径不是 MVP 主验收标准。
- `manifest.circuit.k` 不再覆盖 proof setup 得到的 `ctx.k`，只做一致性断言。
- proof generation 已移入 `tauri::async_runtime::spawn_blocking`，避免卡住 Tauri async runtime。

本轮已经完成主要代码落地和目录迁移：

- `halo2-verifier.move`：新增 Rust crate `sui-ptb-helper`，承接 Sui PTB chunk upload、local keypair signing 和 transaction submission。
- `tauri-example`：已创建并切到 `feat/mvp` 分支，迁入 GUI 与 `mint-min`，Tauri backend 改为 Rust direct API 路径。
- `zkmove-vm`：删除新加的 `cli/src/api/sui.rs` 和 `cli/examples/`，保留既有 CLI `commands/sui.rs` 兼容面。
- `zkmove`：原 `examples/confidential-asset/gui` 和 `mint-min` 已从源 repo 移除；`off-chain` circuit package 暂时仍留在 `zkmove`。

核心路径从：

```text
GUI -> shell sdk -> zkmove CLI -> upload_sui_*.sh -> sui client call
```

调整为：

```text
GUI -> zkmove_cli::api -> demo PTB mint builder -> sui_ptb_helper -> Rust Sui SDK signing/submission
```

## 2. 分仓库改动

### `/Users/ssyuan/work/project/halo2-verifier.move`

新增/修改：

- `Cargo.toml`
  - workspace members 增加 `crates/sui-ptb-helper`。
  - workspace dependencies 增加 `sui-ptb-helper`。
- `crates/sui-verifier-api/src/artifact.rs`
  - 新增 `Digest32`。
  - 新增 `digest32(bytes)`，使用 Blake2b-256。
  - 新增 `ChunkPlan`，默认 chunk size 为 `15360` bytes。
- `crates/sui-verifier-api/src/lib.rs`
  - 导出 `artifact` module。
- `crates/sui-ptb-helper/`
  - 新增 `SuiPtbConfig`。
  - 新增 `SuiPtbClient::connect`。
  - 新增 `SuiPtbClient::execute_ptb`，执行调用方构造好的 `ProgrammableTransaction`。
  - 新增 `SuiPtbClient::object_input`，把已有 object id 解析为 PTB input argument。
  - 新增 `add_new_proof_builder_call`、`add_append_chunk_call`、`add_move_call` 这类通用 PTB command helper。
  - 新增 `move_call`，通过 Rust Sui SDK 构造、签名、提交 transaction。
  - 保留 `upload_proof`，作为 multi-transaction fallback，不作为 demo mint 主路径。
  - 新增 `upload_verifier_artifacts`，覆盖 params/vk/circuit-info builder upload 与 finalize。
  - 新增 `read_object_field_json`，并保留 `read_object_field` 作为 GUI 读取 `Store.balance` 的字符串便捷接口。
  - 内部解析 `object_changes` 里的 created object id，替代 shell `jq`。
- `packages/api-sui/examples/chunked_publish_ptb.md`
  - 修正旧文档中独立 `SerializedCircuit` 的描述。
  - 当前事实源是 `SerializedVK = vk_bytes + circuit_info_bytes`。

注意：

- `scripts/upload_sui_artifacts.sh` 和 `scripts/upload_sui_proof.sh` 还没有删除；当前它们可作为兼容/对照脚本存在。
- helper 目前使用本地 sibling Sui fork path dependency：`../../../zkmove_sui/crates/...`。

### `/Users/ssyuan/work/project/tauri-example`

当前分支：

```bash
feat/mvp
```

新增文件：

- `.gitignore`
- `MVP-RUNBOOK.md`
- `mint-min.manifest.json`
- `mint-min/`
- `src-tauri/`
- `ui/`

关键实现：

- `src-tauri/src/main.rs`
  - 删除 `Command::new(zkmove-mint.sh)` shell 调用。
  - `compute_encrypted` 直接调用 `zkmove_cli::api::poseidon::poseidon_hash`。
  - `prove_mint` 作为诊断命令，直接走 `zkmove_cli::api` 生成 proof 并本地 verify。
  - `mint` 一次完成：
    - 读取 manifest；
    - 计算 encrypted value；
    - 生成 witness；
    - `setup_with_witness`；
    - `prove_with_witness`；
    - 本地 `verify`；
    - 构造单笔 PTB；
    - `artifact_builder::new_proof_builder`；
    - `artifact_builder::append_chunk`；
    - `mint_min::mint_from_builder`；
    - 读取 `Store.balance`。
  - proof 超过单 chunk 时才走 `multi_tx_fallback`。
- `src-tauri/Cargo.toml`
  - 新增依赖 `zkmove-cli` path dependency。
  - 新增依赖 `sui-ptb-helper` path dependency。
- `ui/index.html`
  - 前端只调用 `mint` command，不再在前端传 proof 文件路径或 instance 文件路径。
- `mint-min/Move.toml`
  - `verifier_api` local path 调整为：

```toml
verifier_api = { local = "../../halo2-verifier.move/packages/api-sui" }
```

- `mint-min/run-localnet-e2e.sh`
  - 调整 repo 推导逻辑：
    - `APP_REPO=/Users/ssyuan/work/project/tauri-example`
    - `ZKMOVE_REPO=/Users/ssyuan/work/project/zkmove`
    - `ZKMOVE_VM_REPO=/Users/ssyuan/work/project/zkmove-vm`
    - `HALO2_REPO=/Users/ssyuan/work/project/halo2-verifier.move`

配置：

- `mint-min.manifest.json`
  - 保留 app package/store/verifier object ids。
  - 新增 `sui.rpcUrl`、`sui.keystorePath`、`sui.sender`。
  - 默认 `SUI_RPC_URL` 为 `http://127.0.0.1:9000`。
  - `off-chain` circuit package 改为相对路径：

```bash
../zkmove/examples/confidential-asset/off-chain
```

### `/Users/ssyuan/work/project/zkmove-vm`

完成：

- 删除 `cli/src/api/sui.rs`。
- 删除 `cli/examples/`。
- `cli/src/api/mod.rs` 移除 `pub mod sui;`。

保留：

- `cli/src/commands/sui.rs` 仍存在，作为既有 CLI 兼容面。
- 这符合“删除之前新加的 API/examples，但不误删旧 CLI 命令”的边界。

### `/Users/ssyuan/work/project/zkmove`

完成：

- 删除原未跟踪目录：
  - `examples/confidential-asset/gui/`
  - `examples/confidential-asset/mint-min/`

未移动：

- `examples/confidential-asset/off-chain/`
- `examples/confidential-asset/sdk/`
- `examples/confidential-asset/on-chain*`
- 原文档和其它未跟踪内容

这次只执行团队结论中明确要移动的 `gui` 和 `mint-min`。

## 3. 验证结果

已通过：

```bash
cargo fmt
```

范围：

- `/Users/ssyuan/work/project/halo2-verifier.move`
- `/Users/ssyuan/work/project/tauri-example/src-tauri`

已通过：

```bash
rtk cargo check -p zkmove-cli
```

结果：`zkmove-cli` 编译检查通过，删除 `api/sui.rs` 未破坏 CLI。

已通过：

```bash
/Users/ssyuan/work/project/zkmove_sui/target/debug/sui move build --build-env testnet --silence-warnings
```

工作目录：

```bash
/Users/ssyuan/work/project/tauri-example/mint-min
```

结果：`mint_min` 构建通过，迁移后的 `verifier_api` relative path 有效。

已通过：

```bash
python3 -m json.tool /Users/ssyuan/work/project/tauri-example/mint-min.manifest.json
```

结果：manifest JSON 语法有效。

已通过：

```bash
git diff --check
```

范围：

- `tauri-example`
- `halo2-verifier.move`
- `zkmove-vm`

结果：未发现 whitespace/error marker 问题。

未通过但原因是外部网络阻塞：

```bash
rtk cargo test -p sui-verifier-api
rtk cargo test -p sui-ptb-helper
rtk cargo check --manifest-path /Users/ssyuan/work/project/tauri-example/src-tauri/Cargo.toml
```

失败原因一致：

```text
failed to connect to index.crates.io
download of config.json failed
```

即使提权重跑，也没有进入 Rust 编译阶段。因此目前不能宣称 `sui-ptb-helper` 和 Tauri backend 已完成编译验证。

## 4. 当前风险与待 Review 点

建议同事重点 review：

1. `sui-ptb-helper` 的 repo 归属是否接受。
   - 当前放在 `halo2-verifier.move/crates`。
   - 它依赖本地 Sui fork path。
   - 如果未来发布 crate，需要重新设计 dependency strategy。

2. `SuiPtbClient::move_call` 当前仅作为 fallback/兼容接口。
   - 主验收路径必须走 `execute_ptb`。
   - 如果结果里 `txMode = multi_tx_fallback`，只能说明兼容路径可用，不能作为“PTB 主路径已通过”的证据。

3. `upload_verifier_artifacts` 是否应该现在接入 GUI/setup flow。
   - 当前 GUI 主路径只需要 `upload_proof`，因为 params/vk object id 来自 manifest。
   - setup tooling 以后可以复用 `upload_verifier_artifacts`。

4. `mint-min.manifest.json` 里的 object/package id 是旧 localnet 示例值。
   - 每次重新 localnet publish 后必须刷新。
   - 不应作为长期默认主网/testnet 配置。

5. `off-chain` package 仍留在 `zkmove`。
   - 这是本轮默认假设。
   - 如果要让 `tauri-example` 完全自包含，需要下一轮把 `off-chain` 也迁过来或做成明确 dependency。

6. `src-tauri/Cargo.lock` 已删除。
   - 原 lock 来自旧 shell 薄壳，依赖已过期。
   - 等网络恢复后应重新运行 `cargo check` 生成新 lock。

7. localnet E2E 尚未重跑。
   - 已验证 `mint-min` Move build。
   - 但 GUI direct path 还没因为 Rust dependency resolution 成功而完整跑通。

8. strong binding 验收依赖 `zkmove-vm` 的 row-mapping 修复。
   - 当前要求 `codex/fix-public-input-row-mapping` 分支，至少包含 `419b000 fix public input row mapping`。
   - 修复未合入 main 前，不能把 MVP 描述为 mainline 可复现。

9. `zkmove-vm` 还有 API export 改动需要固化。
   - `cli/src/api/setup.rs` / `mod.rs` 中的 public export 是 Tauri backend 编译依赖。
   - 不提交这些改动，同事拉取后会编译失败。

## 5. 建议下一步

建议 review 后按顺序处理：

1. 解决 crates.io/GitHub dependency resolution，重新运行：

```bash
cd /Users/ssyuan/work/project/halo2-verifier.move
rtk cargo test -p sui-verifier-api
rtk cargo test -p sui-ptb-helper

cd /Users/ssyuan/work/project/tauri-example
rtk cargo check --manifest-path src-tauri/Cargo.toml
```

2. 如果编译暴露 Sui SDK API 细节错误，优先修 `sui-ptb-helper`，再修 Tauri backend。

3. 运行 localnet E2E：

```bash
RUN_ID=mint-min-tauri-example \
RPC_PORT=9011 \
FAUCET_PORT=9134 \
/Users/ssyuan/work/project/tauri-example/mint-min/run-localnet-e2e.sh 6 0
```

4. E2E 成功后，把新的 package/object ids 写回：

```bash
/Users/ssyuan/work/project/tauri-example/mint-min.manifest.json
```

5. 再启动 GUI，验证点击 `Mint` 能完整完成：

```bash
cd /Users/ssyuan/work/project/tauri-example/src-tauri
cargo run
```

6. 最后补一个 negative case 自动化：
   - 使用同一个 proof；
   - 将 `encrypted_amount + 1` 传给 `mint_min::mint`；
   - 预期 abort `EInvalidProof = 1`；
   - balance 不变。

## 6. 当前文件状态摘要

`tauri-example`：

```text
branch: feat/mvp
new files:
  .gitignore
  MVP-RUNBOOK.md
  mint-min.manifest.json
  mint-min/
  src-tauri/
  ui/
```

`halo2-verifier.move`：

```text
modified:
  Cargo.toml
  crates/sui-verifier-api/Cargo.toml
  crates/sui-verifier-api/src/lib.rs
  packages/api-sui/examples/chunked_publish_ptb.md

new:
  crates/sui-verifier-api/src/artifact.rs
  crates/sui-ptb-helper/
```

`zkmove-vm`：

```text
modified:
  cli/src/api/mod.rs

removed:
  cli/src/api/sui.rs
  cli/examples/
```

`zkmove`：

```text
removed from source location:
  examples/confidential-asset/gui/
  examples/confidential-asset/mint-min/
```
