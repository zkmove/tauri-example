# zkMove Confidential Mint MVP

这个仓库是 `zkMove Confidential Mint` 的 MVP 应用。它包含一个 Tauri GUI backend、一个最小 Sui Move package `mint-min`，以及用于 localnet 验收的 single PTB harness。

MVP 验证的核心语义是：

1. 用户本地知道 `(value, nonce)`。
2. `encrypted_amount = poseidon(value, nonce)`。
3. proof 将 `encrypted_amount` 作为 public input 暴露，也就是 `pubs_indices = [1]`。
4. 链上 `mint_min::mint_from_builder` 根据 `encrypted_amount` 重建 public inputs。
5. proof 通过 `artifact_builder::verify_proof_from_builder` 后才 mint。
6. 用 A amount 的 proof 去 mint B amount 必须失败。

> 重要：仓库里的 `mint-min.manifest.json` 只能当作样例。localnet 重启、重新 publish、重新 register、重新上传 params/vk 后，`store`、`appPackage`、`verifierApiPackage`、`paramsObjectId`、`vkObjectId` 都可能失效。`unable to fetch object ...` 基本就是 manifest 里的 object id 指向了旧 localnet。

## 0. 终端约定

下面所有命令默认在同一个 shell 里连续执行。不要跳过 `export`，后续步骤会复用这些变量。

默认假设几个 repo 放在同一个父目录下；如果你的目录不同，只需要改这里：

```bash
export WORK_ROOT="$HOME/work/project"
export APP_REPO="$WORK_ROOT/tauri-example"
export ZKMOVE_REPO="$WORK_ROOT/zkmove"
export ZKMOVE_VM_REPO="$WORK_ROOT/zkmove-vm"
export HALO2_REPO="$WORK_ROOT/halo2-verifier.move"
export SUI_REPO="$WORK_ROOT/zkmove_sui"

export SUI_BIN="$SUI_REPO/target/debug/sui"
export MOVE_BIN="${MOVE_BIN:-move}"
export OFFCHAIN_PACKAGE="$ZKMOVE_REPO/examples/confidential-asset/off-chain"
export PARAMS_PATH="$HALO2_REPO/example/params/kzg_bn254_12.srs"
```

确认关键文件存在时，不要只跑 `test -f` 或 `test -x`，因为成功和失败都不会输出文本。用下面这个带输出的检查：

```bash
check_file() {
  if test -f "$1"; then
    echo "OK file $1"
  else
    echo "MISSING file $1"
    return 1
  fi
}

check_exec() {
  if test -x "$1"; then
    echo "OK executable $1"
  else
    echo "MISSING executable $1"
    return 1
  fi
}

check_cmd() {
  if command -v "$1" >/dev/null 2>&1; then
    echo "OK command $1"
  else
    echo "MISSING command $1"
    return 1
  fi
}

check_cmd jq
check_cmd curl
check_cmd "$MOVE_BIN"
check_file "$APP_REPO/backend/Cargo.toml"
check_file "$APP_REPO/mint-min/Move.toml"
check_file "$ZKMOVE_VM_REPO/cli/Cargo.toml"
check_file "$OFFCHAIN_PACKAGE/Move.toml"
check_file "$PARAMS_PATH"
check_exec "$SUI_BIN"
```

如果这里输出 `MISSING executable $SUI_BIN`，先跳到 [常见问题：`sui` 不存在](#sui-不存在) 处理，再回到这里继续。

backend 依赖 `zkmove-cli` library，不要求预先存在 `zkmove` CLI binary。需要确认 `zkmove-cli` 能编译时使用：

```bash
cd "$ZKMOVE_VM_REPO"
cargo check -p zkmove-cli
```

## 1. 启动 fresh localnet 并创建账户

这一步会在 `$APP_REPO/.localnet/<run-id>` 下创建本次 run 私有的 Sui client config 和 keystore，不会要求你去全局目录里找 `sui.keystore`。

```bash
export RUN_ID="${RUN_ID:-mint-min-$(date +%Y%m%d-%H%M%S)}"
export RUN_DIR="$APP_REPO/.localnet/$RUN_ID"
export RPC_HOST="${RPC_HOST:-127.0.0.1}"
export RPC_PORT="${RPC_PORT:-9000}"
export FAUCET_PORT="${FAUCET_PORT:-9123}"
export SUI_RPC_URL="http://$RPC_HOST:$RPC_PORT"
export SUI_FAUCET_URL="http://$RPC_HOST:$FAUCET_PORT/gas"
export SUI_CLIENT_CONFIG="$RUN_DIR/sui/client.yaml"
export SUI_PUBFILE="$RUN_DIR/Pub.localnet.toml"

mkdir -p "$RUN_DIR/sui" "$RUN_DIR/logs"

SUI_CONFIG_DIR="$RUN_DIR/sui" "$SUI_BIN" start \
  --force-regenesis \
  --fullnode-rpc-port "$RPC_PORT" \
  --with-faucet="$RPC_HOST:$FAUCET_PORT" \
  >"$RUN_DIR/logs/localnet.log" 2>&1 &

export LOCALNET_PID=$!
echo "localnet pid = $LOCALNET_PID"
```

等 RPC ready：

```bash
until curl -sS \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"sui_getChainIdentifier","params":[]}' \
  "$SUI_RPC_URL" | jq -e '.result' >/dev/null; do
  sleep 1
done
```

创建本次 run 的 localnet 环境、账户、gas：

```bash
"$SUI_BIN" client --client.config "$SUI_CLIENT_CONFIG" -y new-env \
  --alias localnet \
  --rpc "$SUI_RPC_URL"

"$SUI_BIN" client --client.config "$SUI_CLIENT_CONFIG" switch --env localnet

export SUI_SENDER="$("$SUI_BIN" client --client.config "$SUI_CLIENT_CONFIG" --json -q new-address ed25519 mint-min-user | jq -r '.address')"
"$SUI_BIN" client --client.config "$SUI_CLIENT_CONFIG" switch --address "$SUI_SENDER"

until "$SUI_BIN" client --client.config "$SUI_CLIENT_CONFIG" faucet \
  --address "$SUI_SENDER" \
  --url "$SUI_FAUCET_URL"; do
  sleep 1
done

"$SUI_BIN" client --client.config "$SUI_CLIENT_CONFIG" balance "$SUI_SENDER"
```

给 GUI backend 和 harness 使用的两个值就在这里：

```bash
export SUI_KEYSTORE_PATH="$(sed -n 's/^  File: //p' "$SUI_CLIENT_CONFIG" | head -1)"
export SUI_SENDER="$("$SUI_BIN" client --client.config "$SUI_CLIENT_CONFIG" active-address | tail -n 1 | tr -d '[:space:]')"

echo "SUI_KEYSTORE_PATH=$SUI_KEYSTORE_PATH"
echo "SUI_SENDER=$SUI_SENDER"
test -f "$SUI_KEYSTORE_PATH" && echo "OK keystore" || echo "MISSING keystore"
```

## 2. 准备 GUI setup 输入

下面用默认验收值：

```bash
export VALUE="${VALUE:-6}"
export NONCE="${NONCE:-0}"
```

构建 off-chain circuit package：

```bash
cd "$OFFCHAIN_PACKAGE"
"$MOVE_BIN" build --skip-fetch-latest-git-deps
```

从这里开始，不再用 `zkmove vm dry-run/setup/prove/verify` 生成本次 run 的 setup artifacts；GUI backend 会直接调用 Rust API 完成 witness、setup、proof/local verify。

## 3. 发布 verifier API，并用 GUI 上传 params/vk

如果你是在同一个 `$RUN_DIR` 里重跑第 3 步，先换一组新的 step-local pubfile；不要手工编辑 `Pub.localnet.toml`：

```bash
export SETUP_ATTEMPT_ID="$(date +%Y%m%d-%H%M%S)"
export SUI_PUBFILE="$RUN_DIR/Pub.$SETUP_ATTEMPT_ID.localnet.toml"
```

发布 `verifier_api` 到当前 localnet：

```bash
if ! "$SUI_BIN" client --client.config "$SUI_CLIENT_CONFIG" --json -q test-publish \
  --build-env testnet \
  --pubfile-path "$SUI_PUBFILE" \
  --skip-dependency-verification \
  --gas-budget 1000000000 \
  "$HALO2_REPO/packages/api-sui" \
  >"$RUN_DIR/verifier-api.publish.json" \
  2>"$RUN_DIR/verifier-api.publish.err"; then
  cat "$RUN_DIR/verifier-api.publish.err"
  cat "$RUN_DIR/verifier-api.publish.json"
  exit 1
fi

jq empty "$RUN_DIR/verifier-api.publish.json"

export VERIFIER_API_PACKAGE="$(jq -r 'first(.objectChanges[]? | select(.type == "published") | .packageId) // empty' "$RUN_DIR/verifier-api.publish.json")"
echo "VERIFIER_API_PACKAGE=$VERIFIER_API_PACKAGE"
test -n "$VERIFIER_API_PACKAGE"
```

创建一个本次 run 的 manifest。这里 `appPackage`、`store`、`paramsObjectId`、`vkObjectId` 先留空；GUI setup 会写回 `paramsObjectId`、`vkObjectId` 和 `circuit.k`，第 5 步会补上 `appPackage` 和 `store`：

```bash
export ZKMOVE_MINT_MANIFEST="$RUN_DIR/mint-min.manifest.json"

cat >"$ZKMOVE_MINT_MANIFEST" <<EOF
{
  "chain": "sui-localnet",
  "appPackage": "",
  "module": "mint_min",
  "function": "mint_from_builder",
  "store": "",
  "verifierApiPackage": "$VERIFIER_API_PACKAGE",
  "paramsObjectId": "",
  "vkObjectId": "",
  "gasBudget": "1000000000",
  "sui": {
    "rpcUrl": "$SUI_RPC_URL",
    "keystorePath": "$SUI_KEYSTORE_PATH",
    "sender": "$SUI_SENDER"
  },
  "circuit": {
    "packagePath": "$OFFCHAIN_PACKAGE",
    "moduleStorage": "storage/0x000000000000000000000000000000000000000000000000000000000000cafe/modules/encryption.mv",
    "entryFunction": "encrypt",
    "circuitName": "encrypt",
    "pubsIndices": [1],
    "kzg": "gwc",
    "k": null,
    "srsPath": "$PARAMS_PATH"
  }
}
EOF

jq . "$ZKMOVE_MINT_MANIFEST"
```

启动 GUI，并在 `Setup verifier artifacts` 区域输入同一个 `VERIFIER_API_PACKAGE`。`manifestPath` 可以留空，此时 backend 使用启动时的 `ZKMOVE_MINT_MANIFEST`；也可以直接粘贴 `$ZKMOVE_MINT_MANIFEST`。`amount` 和 `nonce` 使用上面导出的 `$VALUE` / `$NONCE`，默认就是 `6` / `0`。点击 `Setup Verifier Artifacts` 后，backend 会调用 Rust API 生成 witness/setup/proof/local verify，再把 compact verifier params、vk 和 circuit info 通过 builder/chunk/finalize 多笔交易上传，最后写回 manifest：

```bash
cd "$APP_REPO/backend"
ZKMOVE_MINT_MANIFEST="$ZKMOVE_MINT_MANIFEST" \
SUI_RPC_URL="$SUI_RPC_URL" \
SUI_KEYSTORE_PATH="$SUI_KEYSTORE_PATH" \
SUI_SENDER="$SUI_SENDER" \
cargo run --bin zkmove-mint-gui
```

GUI setup 成功后，先关闭 GUI 窗口或在终端里停止 `cargo run`，再回到同一个 shell 确认 manifest 已经被写回：

```bash
export PARAMS_OBJECT_ID="$(jq -r '.paramsObjectId' "$ZKMOVE_MINT_MANIFEST")"
export VK_OBJECT_ID="$(jq -r '.vkObjectId' "$ZKMOVE_MINT_MANIFEST")"
export K_VALUE="$(jq -r '.circuit.k' "$ZKMOVE_MINT_MANIFEST")"

echo "PARAMS_OBJECT_ID=$PARAMS_OBJECT_ID"
echo "VK_OBJECT_ID=$VK_OBJECT_ID"
echo "K_VALUE=$K_VALUE"
test -n "$PARAMS_OBJECT_ID" && test "$PARAMS_OBJECT_ID" != "null"
test -n "$VK_OBJECT_ID" && test "$VK_OBJECT_ID" != "null"
test -n "$K_VALUE" && test "$K_VALUE" != "null"
```

## 4. 发布 `mint-min` 并创建 Store

构建并发布 `mint-min`：

```bash
cd "$APP_REPO/mint-min"
"$SUI_BIN" move build --build-env testnet --silence-warnings

if ! "$SUI_BIN" client --client.config "$SUI_CLIENT_CONFIG" --json -q test-publish \
  --build-env testnet \
  --pubfile-path "$SUI_PUBFILE" \
  --skip-dependency-verification \
  --gas-budget 1000000000 \
  "$APP_REPO/mint-min" \
  >"$RUN_DIR/mint-min.publish.json" \
  2>"$RUN_DIR/mint-min.publish.err"; then
  cat "$RUN_DIR/mint-min.publish.err"
  cat "$RUN_DIR/mint-min.publish.json"
  exit 1
fi

jq empty "$RUN_DIR/mint-min.publish.json"

export APP_PACKAGE="$(jq -r 'first(.objectChanges[]? | select(.type == "published") | .packageId) // empty' "$RUN_DIR/mint-min.publish.json")"
echo "APP_PACKAGE=$APP_PACKAGE"
test -n "$APP_PACKAGE"
```

创建本次 run 的 `mint_min::Store`。这一步就是 `manifest.store` 的来源：

```bash
"$SUI_BIN" client --client.config "$SUI_CLIENT_CONFIG" --json -q call \
  --package "$APP_PACKAGE" \
  --module mint_min \
  --function register \
  --gas-budget 1000000000 \
  >"$RUN_DIR/register.json"

export STORE_OBJECT_ID="$(jq -r '
  first(
    .objectChanges[]?
    | select(.type == "created")
    | select((.objectType // "") | endswith("::mint_min::Store"))
    | .objectId
  ) // empty
' "$RUN_DIR/register.json")"

echo "STORE_OBJECT_ID=$STORE_OBJECT_ID"
test -n "$STORE_OBJECT_ID"
```

如果后面看到 `unable to fetch object <store-id>`，先用这个命令确认：

```bash
"$SUI_BIN" client --client.config "$SUI_CLIENT_CONFIG" object "$STORE_OBJECT_ID" --json | jq '.objectId // .data.objectId'
```

如果这里查不到，说明 `STORE_OBJECT_ID` 已经不是当前 localnet 上的 object，需要重新执行本节的 `register`，并重写 manifest。

## 5. 补全本次 run 的 manifest

第 3 步已经创建并写回了 `$ZKMOVE_MINT_MANIFEST`。这里只补上第 4 步得到的 `APP_PACKAGE` 和 `STORE_OBJECT_ID`：

```bash
tmp_manifest="$ZKMOVE_MINT_MANIFEST.tmp"
jq \
  --arg app "$APP_PACKAGE" \
  --arg store "$STORE_OBJECT_ID" \
  '.appPackage = $app | .store = $store' \
  "$ZKMOVE_MINT_MANIFEST" >"$tmp_manifest"
mv "$tmp_manifest" "$ZKMOVE_MINT_MANIFEST"

jq . "$ZKMOVE_MINT_MANIFEST"
```

提交前先做一次类型一致性检查。这里要全部输出 `OK`；如果 `params` 或 `vk` 显示的 package 不是当前 `VERIFIER_API_PACKAGE`，第 6 步会在链上报 `TypeMismatch`：

```bash
sui_object_type_package() {
  "$SUI_BIN" client --client.config "$SUI_CLIENT_CONFIG" object "$1" --json | jq -er '
    (
      .data.type
      // .data.objectType
      // .type
      // .objectType
      // .details.data.type
      // .details.data.objectType
      // .result.data.type
      // .result.data.objectType
    )
    | split("::")[0]
  '
}

export PARAMS_TYPE_PACKAGE="$(sui_object_type_package "$PARAMS_OBJECT_ID")"
export VK_TYPE_PACKAGE="$(sui_object_type_package "$VK_OBJECT_ID")"

export STORE_TYPE_PACKAGE="$(jq -r --arg id "$STORE_OBJECT_ID" '
  first(.objectChanges[]? | select(.type == "created" and .objectId == $id) | .objectType)
  | split("::")[0]
' "$RUN_DIR/register.json")"

test "$PARAMS_TYPE_PACKAGE" = "$VERIFIER_API_PACKAGE" && echo "OK params type" || {
  echo "BAD params type: $PARAMS_TYPE_PACKAGE != $VERIFIER_API_PACKAGE"
  exit 1
}

test "$VK_TYPE_PACKAGE" = "$VERIFIER_API_PACKAGE" && echo "OK vk type" || {
  echo "BAD vk type: $VK_TYPE_PACKAGE != $VERIFIER_API_PACKAGE"
  exit 1
}

test "$STORE_TYPE_PACKAGE" = "$APP_PACKAGE" && echo "OK store type" || {
  echo "BAD store type: $STORE_TYPE_PACKAGE != $APP_PACKAGE"
  exit 1
}
```

写一份 `summary.env`，如果你换了一个新终端，可以直接 `source` 回来：

```bash
cat >"$RUN_DIR/summary.env" <<EOF
export RUN_DIR="$RUN_DIR"
export LOCALNET_PID="$LOCALNET_PID"
export SETUP_ATTEMPT_ID="$SETUP_ATTEMPT_ID"
export SUI_RPC_URL="$SUI_RPC_URL"
export SUI_CLIENT_CONFIG="$SUI_CLIENT_CONFIG"
export SUI_PUBFILE="$SUI_PUBFILE"
export SUI_KEYSTORE_PATH="$SUI_KEYSTORE_PATH"
export SUI_SENDER="$SUI_SENDER"
export VERIFIER_API_PACKAGE="$VERIFIER_API_PACKAGE"
export PARAMS_OBJECT_ID="$PARAMS_OBJECT_ID"
export VK_OBJECT_ID="$VK_OBJECT_ID"
export APP_PACKAGE="$APP_PACKAGE"
export STORE_OBJECT_ID="$STORE_OBJECT_ID"
export ZKMOVE_MINT_MANIFEST="$ZKMOVE_MINT_MANIFEST"
export VALUE="$VALUE"
export NONCE="$NONCE"
EOF

source "$RUN_DIR/summary.env"
```

## 6. 运行 localnet PTB harness

PTB 主验收使用 harness，不使用旧的 `upload_sui_proof.sh` 多交易路径：

```bash
cd "$APP_REPO"
ZKMOVE_MINT_MANIFEST="$ZKMOVE_MINT_MANIFEST" \
  ./mint-min/run-localnet-ptb-harness.sh --value "$VALUE" --nonce "$NONCE" --negative
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

## 7. 运行 GUI

同一个 shell 里保留 `ZKMOVE_MINT_MANIFEST`、`SUI_KEYSTORE_PATH`、`SUI_SENDER` 后启动 GUI；如果你不是从同一个 shell 启动，就在 GUI 的 `manifestPath` 输入框里粘贴 `$ZKMOVE_MINT_MANIFEST`：

```bash
cd "$APP_REPO/backend"
cargo run --bin zkmove-mint-gui
```

在窗口中输入 `amount` 和 `nonce` 后点击 `Mint`。本地会生成 witness/proof，先本地 verify，再用 single PTB 提交链上 mint。

成功结果应包含：

```text
tx mode = single_ptb
tx <digest> -> success
```

## 8. 验收检查

推荐按下面顺序检查：

```bash
cd "$HALO2_REPO"
cargo test -p sui-verifier-api
cargo test -p sui-ptb-helper

cd "$ZKMOVE_VM_REPO"
cargo check -p zkmove-cli

cd "$APP_REPO"
cargo check --manifest-path backend/Cargo.toml
ZKMOVE_MINT_MANIFEST="$ZKMOVE_MINT_MANIFEST" \
  ./mint-min/run-localnet-ptb-harness.sh --value "$VALUE" --nonce "$NONCE" --negative
```

最终验收标准：

1. `sui-verifier-api` tests 通过。
2. `sui-ptb-helper` tests 通过。
3. `zkmove-cli` check 通过。
4. `backend` check 通过。
5. positive mint 输出 `txMode=single_ptb`，且 `Store.balance == encrypted_amount`。
6. negative mint 输出 `txMode=single_ptb status=aborted`，且 balance 不变。

## 9. 常见问题

`sui` 不存在，或第 0 步输出 `MISSING executable $SUI_BIN`：

- 默认路径假设 Sui repo 在 `$WORK_ROOT/zkmove_sui`，如果你的目录不同，先修正第 0 步里的 `SUI_REPO`。
- 确认目录正确后构建 `sui` binary：

```bash
cd "$SUI_REPO"
cargo build --bin sui
```

- 构建成功后回到第 0 步，重新执行 `check_exec "$SUI_BIN"`，应该输出 `OK executable ...`。

`error: unable to fetch object 0x...`

- 优先检查报错里的 object id 是不是 manifest 里的 `store`、`paramsObjectId` 或 `vkObjectId`。
- 如果是 `store`，重新执行 `mint_min::register` 并更新 manifest 的 `store`。
- 如果 localnet 是重新启动过的，不要只改 `store`；重新发布 `verifier_api`、在 GUI 里重新 setup/upload params/vk、重新发布 `mint-min`、重新 register，然后补全 manifest。

`jq: parse error: Invalid numeric literal` 且 publish 输出里出现 `Ephemeral publication file "Pub.localnet.toml" has chain-id ...`：

- 这是 `mint-min/Pub.localnet.toml` 记录了旧 localnet 的 chain id。
- 重新跑 README 里的 publish 命令时必须带 `--pubfile-path "$SUI_PUBFILE"`。
- `$SUI_PUBFILE` 指向 `$RUN_DIR/Pub.<attempt>.localnet.toml`，每次 fresh localnet 都是独立文件，不会复用 package 目录里的旧 `Pub.localnet.toml`。
- 如果已经在当前 run 里遇到这个错误，设置一个新的 `SUI_PUBFILE`，然后回到第 3 步从“发布 `verifier_api` 到当前 localnet”开始重新执行；不要只重跑 `mint-min` publish，因为 `mint-min`、`paramsObjectId`、`vkObjectId` 必须绑定到同一个 `verifierApiPackage`。

`Failed to publish ... Your package is already published`：

- 这是当前 `$SUI_PUBFILE` 里已经有相同 package 的 `[[published]]` 记录。
- 设置一个新的 pubfile 后，从第 3 步重新跑：

```bash
export SUI_PUBFILE="$RUN_DIR/Pub.$(date +%Y%m%d-%H%M%S).localnet.toml"
```

第 6 步报 `CommandArgumentError { arg_idx: 1, kind: TypeMismatch }`：

- `arg_idx: 1` 是 `mint_min::mint_from_builder` 的第二个参数 `params`。
- 这通常说明 `paramsObjectId` / `vkObjectId` 来自旧的 `verifier_api` package，但 `mint-min` 是按新的 `verifier_api` dependency publish 的。
- 回到第 3 步，设置新的 `SETUP_ATTEMPT_ID` 和 `SUI_PUBFILE`，重新发布 `verifier_api`，在 GUI 里重新执行 `Setup Verifier Artifacts`，然后完整重跑第 4、5 步。第 5 步的类型一致性检查必须全部输出 `OK` 后再跑第 6 步。

第 5 步类型检查显示 `BAD params type: null != ...`：

- 先确认 shell 里的 `ZKMOVE_MINT_MANIFEST` 是 GUI setup 刚写回的文件：

```bash
echo "$ZKMOVE_MINT_MANIFEST"
jq -r '.verifierApiPackage, .paramsObjectId, .vkObjectId' "$ZKMOVE_MINT_MANIFEST"
```

- 如果 `paramsObjectId` 或 `vkObjectId` 是空值，回到第 3 步重新执行 GUI setup，并确认 GUI 的 `manifestPath` 指向同一个 `$ZKMOVE_MINT_MANIFEST`。
- 如果 object id 有值但类型读成 `null`，使用 README 当前版本里的 `sui_object_type_package` helper 重新跑第 5 步类型检查；旧的 `jq '.data.type'` 只兼容部分 `sui client object --json` 输出格式。

`localnet_ptb_harness` 找不到 manifest：

- 确认 `ZKMOVE_MINT_MANIFEST` 是否指向存在的 JSON 文件。
- 未设置时默认使用当前 repo 下的 `mint-min.manifest.json`，这个文件可能是旧 localnet 的样例。

GUI setup 报 `verifier artifacts uploaded, but failed to write manifest ... manual recovery ...`：

- 这说明链上 `params/vk/circuit_info` 已经上传成功，但本地 manifest 文件写回失败。
- 按错误里的 `manual recovery` 字段手动写回 `verifierApiPackage`、`paramsObjectId`、`vkObjectId` 和 `circuit.k`，然后继续第 4、5 步。
- 如果不想手动恢复，也可以修复 manifest 路径或文件权限后回到第 3 步重新执行 GUI setup；旧的链上 artifact object 会留在 localnet 上，不会自动删除。

GUI setup 报 `failed to upload verifier artifacts with verifierApiPackage ...`：

- 先看 GUI 里后续的完整错误链；当前 backend 会显示最内层原因。
- 如果看到 `params artifact is 524548 bytes, exceeds max size 245760`，说明你运行的是旧 backend，它把完整 SRS 当作链上 params 上传了。停止 GUI，确认代码已更新后重新执行第 3 步里的 `cargo run --bin zkmove-mint-gui`。
- 正确路径只上传 compact verifier params，`kzg_bn254_12.srs` 仍然是本地 setup/prove 使用的文件，不会原样写到链上。

交易签名或 gas 报错：

- 确认 `SUI_KEYSTORE_PATH` 来自 `$SUI_CLIENT_CONFIG` 的 `keystore.File`。
- 确认 `SUI_SENDER` 是 `"$SUI_BIN" client --client.config "$SUI_CLIENT_CONFIG" active-address` 输出的地址。
- 确认 `"$SUI_BIN" client --client.config "$SUI_CLIENT_CONFIG" balance "$SUI_SENDER"` 有 gas。

链上 proof verification 失败：

- 确认 GUI setup、backend/harness 都使用 `pubsIndices = [1]`。
- 确认 GUI setup 使用的 `value`/`nonce` 可以生成和 mint 相同 circuit shape。
- 确认 manifest 中的 `circuit.k` 是 GUI setup 写回的值。
- 确认当前 `zkmove-vm` 分支包含 public-input row mapping 修复。

proof 过大：

- 当前 MVP 只接受 single PTB 路径。
- proof 超过当前 single PTB 限制时会 fail closed，不会降级到非原子多交易路径。

停止 localnet：

```bash
kill "$LOCALNET_PID"
```

## Known Limitations

- strong binding 依赖 `zkmove-vm` 的 public-input row mapping 修复；当前本地验证分支是 `feat/mvp`，至少需要包含 `75c0c87 Fix public input instance row mapping for non-zero argument indices (#361)`。该修复未合入 main 前，不能声称“任意同事拉 main 即可复现强绑定验收”。
- Tauri backend 依赖 `zkmove-vm/cli/src/api/setup.rs` 和 `mod.rs` 中的 `setup_with_witness` public export 改动；当前本地验证分支是 `feat/mvp`，至少需要包含 `32f9be4 Expose setup API for mint MVP`。这些改动必须提交或固定分支。
- empty public inputs 只能作为 smoke path，不能作为最终 MVP 验收。
- package publish/register 仍是终端 setup；GUI 目前只覆盖 witness/setup/proof、本地 verify、`params/vk/circuit_info` upload 和 manifest writeback。
