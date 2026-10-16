# Spike：MessagePort 如何抵达 Tauri webview（M3 Task 0）

状态：**已完成（路线 1′ 胜出，渲染层无需改动）**。
上游权威：`docs/plans/2026-10-16-tauri-m3-ecosystem.md`（DR-M3-3）、
`docs/specs/tauri-platform-commands.md` §4.2/§5。
纪律：`docs/specs/clean-room-policy.md`。取证范围与环境见文末「纪律与取证边界」。

**Task 1 可以据此开工**：Host 侧用一条 stdio 长度前缀帧接 `ChannelServer`，壳侧用「Rust 建窗 +
init 脚本在页面内建 `MessageChannel` + 自定义 command + raw 二进制泵」把端口送进 webview，
渲染层启动链一行都不改。

---

## 1. 结论（选定的路线与为什么）

**选定：DR-M3-3 路线 1 的可用形态（下文记作 1′）**，理由是它同时满足「最少的可动面」与
「渲染层零改动」：

> **壳在 Rust 侧建窗并挂 `initialization_script`；该脚本在页面（文档创建时、页面任何脚本之前）
> 内建 `new MessageChannel()`；`port1` 用 `window.postMessage(payload, "*", [port1])` 交给渲染层
> （与 Electron preload 完全同一条路径，渲染层 `event.ports[0]` 拿到的是**真 `MessagePort`**）；
> `port2` 由同一脚本桥到壳（出方向 `invoke(cmd, ArrayBuffer)`，入方向长轮询取 `Response` 的 raw
> bytes）；壳再把字节经 stdio 转发给 Node Host。**

**DR-M3-3 路线 1 的字面机制不可行**，这一条是实测结论而不是推测：Tauri 2.12.4 **没有任何
「Rust 侧造出一个 webview 里的端口对端」的能力**（证据见 §2.2）。可行形态把「谁建端口」换成了
**页面内建**，代价是壳里多一份 JS 桥（在 init 脚本里，因此渲染层仍然一行不改）。

路线 2（壳内桥 + `MessagePortLike` 适配器）**未被实测，也不需要**：它要求渲染层把适配器喂给
`connectViaMessagePort`，即必须动渲染层的调用点；路线 1′ 在不动渲染层的前提下已经跑通，按
DR-M3-3 的排序原则在此止步。路线 3（新增 rpc 传输）同样不需要。

**渲染层是否需要改：不需要。** 见 §5。

---

## 2. 能力取证（先看 API 面，再实测）

证据取自本机 cargo registry：`tauri 2.12.4`、`tauri-utils 2.10.2`、`wry 0.57.0`
（`~/.cargo/registry/src/rsproxy.cn-e3de039b2554c837/`）。

### 2.1 有「页面加载前注入」的能力（问题 a 的前半）

- `WebviewBuilder::initialization_script(script)`：`tauri/src/webview/mod.rs:943`
  （另有 `initialization_script_for_all_frames`，`:1002`）。
- 注入顺序由 `WebviewManager::prepare_pending_webview` 决定（`tauri/src/manager/webview.rs:180-215`）：
  ① `isTauri` / `__TAURI_INTERNALS__` 骨架 → ② `invoke_initialization_script`（invoke 系统）→
  ③ window/webview metadata → ④ `WebviewManager::initialization_script`（pattern/ipc/core/event/
  freeze_prototype，其中含 `withGlobalTauri` 的全局 API 包）→ ⑤ 插件 init 脚本 → ⑥ **调用方自己的
  `initialization_scripts` append 在最后**。
  ⇒ 自定义 init 脚本**看得见 `window.__TAURI_INTERNALS__`**，且在主 frame 的主 world 执行。
  实测印证：注入脚本第一行打印 `injected_ran internals=object invoke=function`。
- **`tauri.conf.json` 的窗口拿不到 init 脚本**：`tauri-utils/src/config.rs` 的 `WindowConfig`
  （`:1952`）没有 `initialization_script` 字段（`grep initialization_script config.rs` 零命中）。
  ⇒ 要用 init 脚本，**窗口必须在 Rust 侧用 `WebviewWindowBuilder` 建**。这是 Task 1 的硬约束。

### 2.2 没有「Rust 侧转移端口」的能力（问题 a 的后半，否定结论）

- `tauri 2.12.4`：`grep -rn "MessagePort|transferable|Transferable" src/ scripts/` **零命中**。
- `wry 0.57.0`：`grep -rn "MessagePort|postMessage" src/lib.rs` 只有两处文档注释，指向
  `window.ipc.postMessage("insert_message_here")`——**字符串**通道；`transferable` 零命中。
- Tauri 的 IPC 面只有四类，没有一类能携带端口：`invoke`（JSON 或整包 raw 字节）、
  `ipc-protocol.js` 的两种投递（`fetch(convertFileSrc(cmd,"ipc"))` / `window.ipc.postMessage(data)`）、
  `Channel`（Rust → JS 推送回调）、`emit/listen`（JSON 事件）。连 Tauri 自己推二进制都是靠
  `webview.eval("new Uint8Array([...]).buffer")` 或 fetch 二次取回
  （`tauri/src/ipc/channel.rs:302` 与 `:421`），**不是原生端口**。

⇒ 「Rust 侧 `postMessage` 一个端口进页面」在 Tauri 2 里做不到。端口只能在页面内建。

### 2.3 页面 ↔ Rust 的二进制通路存在，且是零编码的

- `scripts/process-ipc-message-fn.js`：`message instanceof ArrayBuffer || ArrayBuffer.isView(message)
  || Array.isArray(message)` ⇒ `{ contentType: "application/octet-stream", data: message }`；
  Rust 侧落到 `tauri::ipc::InvokeBody::Raw(Vec<u8>)`（`tauri/src/ipc/mod.rs:59`、`Request::body()` `:155`）。
- 反向：`tauri::ipc::Response::new(bytes)`（`mod.rs:190-202`）回来时 JS 侧走
  `ipc-protocol.js` 的 `default: response.arrayBuffer()`。
- 推模型（Rust 主动推）也在：`Channel<T>` 的发送端同样支持 raw
  （`channel.rs:302` 小包走 `eval`、`:421` 大包走 fetch 取回），阈值
  `MAX_RAW_DIRECT_EXECUTE_THRESHOLD = 1024`、`MAX_JSON_DIRECT_EXECUTE_THRESHOLD = 8192`（`:36`、`:38`）。
  **本 spike 没测这条**（见 §6 的未验证项）；实测的是长轮询模型。

### 2.4 renderer 侧的接收协议形状（只取「消息长什么样」）

| 项 | 形状 | 出处 |
| --- | --- | --- |
| 监听 | `window.addEventListener("message", handler)` | `packages/desktop/src/renderer/src/main.tsx` |
| 端口消息 | `event.source === window && event.data?.type === "mode:service-port"`，端口在 `event.ports[0]`，负载 `{ databaseStartupId }` | 同上 + `packages/shared/src/channels.ts` 的 `InternalChannels.ServicePort` |
| 状态消息 | 同 `source` 条件 + `event.data?.type === "mode:database-startup-state"`，负载 `event.data.state` 过 `databaseStartupStateSchema` | 同上 + `packages/shared/src/database-startup.ts` |
| 准入 | `DatabaseStartupAdmission.acceptState/acceptPort/takeReadyPort`（`phase === "ready"` 且 `pending.startupId === state.startupId` 才交端口） | `packages/desktop/src/renderer/src/databaseStartupAdmission.ts` |
| 连接层 | `connectViaMessagePort(port)` = `new MessagePortProtocol(port)` + `new ChannelClient(protocol)` + `new RemoteServiceAccess(client)` | `packages/client/src/messageport.ts`、`packages/rpc/src/protocol.ts` |
| 端口形状 | `MessagePortLike = { addEventListener, removeEventListener, postMessage, start, close }`；**收帧判定是 `e.data instanceof Uint8Array`** | `packages/rpc/src/protocol.ts` |

**`event.source === window` 是硬条件**：`dispatchEvent(new MessageEvent(...))` 的 `source` 是 `null`，
会被启动链丢掉；必须用页面自身的 `window.postMessage(payload, "*", [port])`（= Electron preload
今天做的那一步）。

---

## 3. 实测：路线 1′ 的完整往返（Step 2）

### 3.1 被测形状

```
Node Host(替身)  --stdio 帧-->  Rust 壳  --raw invoke/长轮询-->  注入桥(页面内)  --MessagePort-->  页面(渲染层启动链)
   ChannelServer                    ↑ raw ArrayBuffer                        port1/port2
```

- 实验壳是 `mode-tauri` crate 里的**临时第二个 bin**（`apps/desktop-tauri/src-tauri/src/bin/port_experiment.rs`，
  已被 §7 的清理删除），窗口用 `WebviewWindowBuilder::initialization_script(...)` 建，
  URL 用 `WebviewUrl::App("experiments/port-transport/index.html")`（**本地内容**，理由见 §4.2）。
- 页面是 spike 专用页（Vite 构建进 `dist/experiments/port-transport/`），**逐字复刻**渲染层的接收形状：
  真 `InternalChannels` 常量、真 `databaseStartupStateSchema`、**未改动的** `DatabaseStartupAdmission`
  源码（直接 import）、真 `MessagePortProtocol` + `ChannelClient`。
- Node 侧是 spike 用的 Host 替身：真 `ChannelServer` + 一个 `spike` 频道（`ping` 方法），
  经 stdio `[4 字节大端长度][payload]` 帧收发。

### 3.2 可复现命令

```bash
# 1) 构建 spike 页（产物落 dist/experiments/port-transport/）
cd apps/desktop-tauri && pnpm exec vite build --config experiments/port-transport/vite.config.ts
# 2) 起诊断用的 dev server（页面/注入桥把日志 fetch 到这里；WebView2 的 console 本机拿不到）
cd apps/desktop-tauri && pnpm exec vite --config experiments/port-transport/vite.config.ts &   # 127.0.0.1:5176
# 3) 构建并运行实验壳
node scripts/rust-cargo.ts build --release -p mode-tauri --bin PortTransportExperiment
./target/release/PortTransportExperiment.exe
# 4) 壳在拿到最终结果后自行退出：0 = 成功，3 = 失败，4 = 30s 看门狗超时
```

> 第 1–3 步的 spike 源码已按计划删除（§7）；重放需要按 §8 的配方重建（约 60 行）。

### 3.3 原始输出（连跑两次，`EXIT=0`）

壳侧 stdout：

```text
SPIKE_INFO host_spawned pid=11160
[spike-host] Initialize sent (pid 11160, node v24.18.1)
SPIKE_REPORT delivered port+state
[spike-host] call ping {"n":41,"text":"from-page"}
SPIKE_REPORT {"status":"ok","portIsRealMessagePort":true,"initializeObserved":true,"rpcCommand":"ping","rpcEcho":{"n":41,"text":"from-page"},"hostPid":11160,"hostVersion":"v24.18.1"}
```

第二次连跑（pid 换成 10236，结论逐字相同，`EXIT=0`）。

页面侧诊断（Vite 进程 stdout，`SPIKE_PAGE_LOG` 前缀）：

```text
bridge=messagechannel_created
bridge=deliver_deferred_to_domcontentloaded
stage=html_inline_ran                        ← 页面 JS 真的跑了
page=main_module_loaded
page=监听已就绪，等待注入桥送来端口与启动状态
bridge=deliver_start readyState=interactive
bridge=deliver_port_posted
bridge=deliver_state_posted
bridge=report delivered port+state
page=收到 service-port 消息：event.source===window=true（不满足会被渲染层启动链直接忽略） event.ports.length=1 真 MessagePort=true
page=收到 database-startup-state（schema 通过=true）
page=admission 交出端口：instanceof MessagePort=true ctor=[object MessagePort]
bridge=pump_first_frame type=[object ArrayBuffer] bytes=6
bridge=pump_first_frame_posted
page=端口收到一帧：[object Uint8Array] bytes=6          ← ChannelServer 的 Initialize
page=ChannelClient 收到 Initialize（第 1 次）
bridge=shell_port_recv bytes=48                          ← 页面的 RPC 请求进壳
page=端口收到一帧：[object Uint8Array] bytes=103         ← ping 的应答
page=REPORT {"status":"ok","portIsRealMessagePort":true,"initializeObserved":true,"rpcCommand":"ping","rpcEcho":{"n":41,"text":"from-page"},...}
```

### 3.4 这一串输出逐条闭合了问题 (a)(b)(c)

| 判据 | 证据行 | 结论 |
| --- | --- | --- |
| 页面加载前注入且能拿到 Tauri IPC | `injected_ran internals=object invoke=function` | (a) 注入**成立**；但「Rust 侧转移端口」**不成立**（§2.2），端口必须页面内建 |
| 注入对象是真 `MessagePort` 而非替身 | `event.source===window=true`、`event.ports.length=1`、`真 MessagePort=true`、`ctor=[object MessagePort]` | (b) **不需要**等价对象，`event.ports` 里就是真端口，渲染层调用点无需改 |
| 端口承载真 `ChannelServer` 的 `Initialize` | `ports 到一帧 Uint8Array bytes=6` + `ChannelClient 收到 Initialize（第 1 次）` | (c) **成立** |
| 端口承载双向 RPC | 请求 48 字节 → Node 侧 `call ping {...}` → 应答 103 字节 → 页面对上 `n:41` | (c) **成立**（不止 Initialize） |

---

## 4. 结构约束（Task 1 必须照做，都是实测出来的）

### 4.1 端口只能在页面内建

见 §2.2。壳能做的是「把页面对端的字节搬到 Host 再搬回来」，不是「把端口递进去」。

### 4.2 渲染层必须由**本地内容**（asset 协议）加载，不能用 http/dev server 供页面

这是两条实测的 ACL 结论，**直接决定 Task 3 的接线方式**：

1. 「窗口 label 不在任何 capability 里」时，**app 自己的 `#[tauri::command]` 会被拒**：
   `bridge=pump_failed Command spike_host_recv not allowed by ACL`。
2. 把窗口加进 capability 之后仍然被拒，因为页面是 `http://127.0.0.1:5176`（**远端内容**）：
   `tauri/src/webview/mod.rs:2077-2098` 的判定是
   `if (plugin_command.is_some() || has_app_acl_manifest || !is_local) && invoke.acl.is_none() → 拒绝`，
   而 app 命令根本不在 `allowed_commands` 里（`gen/schemas/acl-manifests.json` 只有 `core*` 与
   `deep-link`，**没有 app 段**）⇒ **远端来源永远拿不到 app 命令**，加 `remote.urls` 也无用。
   实测报错：`Command spike_report not allowed by ACL`。
3. 改成 `WebviewUrl::App("experiments/port-transport/index.html")`（本地内容）后：
   `SPIKE_REPORT delivered port+state` 立刻通了，且**无需任何 capability 条目**
   （capability 文件已删除即可）。

⇒ Task 3 把 renderer 挂到 `frontendDist` 的方向是对的；**开发期也不要让壳去加载 http 页面**。

### 4.3 二进制要包成 `Uint8Array` 再喂端口（一次静默丢帧的教训）

`MessagePortProtocol` 的收帧判定是 `e.data instanceof Uint8Array`（`packages/rpc/src/protocol.ts`），
而 Tauri 的 raw 响应回来是 **`ArrayBuffer`**。直接 `port.postMessage(arrayBuffer)` 会被协议层
**静默丢弃**：现象是 `端口收到一帧 bytes=6` 但 `ChannelClient 收到 Initialize` 永不出现、RPC 永远挂住。

```js
// 壳 → 页面：必须包一层 Uint8Array
shellPort.postMessage(new Uint8Array(rawFromInvoke));
// 页面 → 壳：MessagePortProtocol.send 发的本来就是 VSBuffer.buffer（Uint8Array），原样透传即可
internals.invoke("host_send", event.data);   // TypedArray 整包 → octet-stream
```

### 4.4 壳与 Host 之间的帧格式

`[4 字节大端长度][payload]` 就够；spike 用 Node 侧长轮询 + `Read::read_exact`。
Task 1 可沿用，也可换成 `child_process.fork` 的 IPC——**这条 spike 没有对二者做取舍**，只证明
「壳 ↔ Host」的字节搬运与「页面 ↔ 壳」解耦，两侧可独立替换。

### 4.5 两个可观测性/健壮性副作用（都记下来免得重走）

- **本机不要 `canonicalize()` 路径再交给 Node**：Windows 上会得到 `\\?\D:\...` 扩展前缀，
  Node 直接 `EISDIR: illegal operation on a directory, lstat 'D:'`（实测）。
- **往 `ChannelServer` 投非 RPC 帧会让 Host 进程崩在 `channelServer.ts` 的 `header[0]` 上**
  （spike 自己用 3 字节探针踩到）。Task 1 的 Host 入口要按帧长度/形状做拒绝，不能假设对端只发合法帧。

---

## 5. 渲染层是否需要改：**不需要**（含未验证边界）

生效的机制让渲染层拿到的是**真 `MessagePort`**，且消息形状（频道名、`event.source`、`event.ports`、
`{ databaseStartupId }`、`startupId` 匹配）与它今天在 Electron 下拿到的**逐字相同**。因此：

- **改动面：零**。`packages/client/**`、`packages/rpc/**`、`packages/shared/**`、
  `packages/services/**`、`packages/desktop/src/renderer/**` 全部未改（见 §7 的提交文件清单）。
- 页面侧那次「Initialize 被丢」的坑是**壳/桥侧**的编码错误（§4.3），不是渲染层要求。

**边界（Task 1/3 仍需证明，本 spike 不含）**：

1. spike 页是**替身**：它复刻了接收形状 + 真 `DatabaseStartupAdmission` + 真 `MessagePortProtocol`/
   `ChannelClient`，但没有 `new RemoteServiceAccess(client)` 之后的 45 个服务面；
   **真实 renderer 挂载（`IServiceAccessor` 有真实现）是 Task 1–3 的事**。
2. `databaseStartupId` / `startupId` 谁生成：spike 里由壳生成并注入字面量，Host 只是接受；
   Task 1 必须定这件事的**唯一所有者**（端口负载的 `databaseStartupId` 必须等于状态流的 `startupId`）。
3. 流控与背压：spike 的泵是无界长轮询，没有用 `MessagePortFlowControl`；Task 1 要设计。
4. 崩溃/重启：Host 死掉时泵会拿到 `read_exact` 错误（spike 里表现为 `读 Host 帧失败`），
   重连策略是 Task 1 的范围。
5. `window.mode`（Electron preload 全局）与 `IPlatformService` 的替换不是新问题，
   M1 已给出 `packages/ui/src/platform/tauri-invoke-shim.ts` 这条路（§4.2 的表）。

---

## 6. 三条路线的对照（Step 2/3/4）

| 路线 | 结论 | 依据 |
| --- | --- | --- |
| **1（字面）Rust 侧转移端口** | **不可行** | §2.2：tauri/wry 都没有端口/transferable 面 |
| **1′（选定）init 脚本在页面内建端口 + 自定义 command 搬运字节** | **可行，已跑通完整往返** | §3 的实测输出；渲染层零改动 |
| **2 壳内桥 + `MessagePortLike` 适配器** | **未实测，且不需要** | 它要求渲染层把适配器喂给 `connectViaMessagePort`，即**必须改渲染层的调用点**；1′ 已在不改渲染层的前提下跑通 |
| 2 的备选传输：Host 起 loopback WebSocket，页面内 `WebSocket` 桥 `port2` | **未实测（评估不采用）** | 好处是不必让壳转发 RPC 字节；代价是多一个监听端口 + 一次性令牌握手 + 与「本地内容」口径无关的额外攻击面。Task 1 若要省掉壳内中继，可再评估 |
| **3 新增 `packages/rpc` 传输** | **不需要** | 1′ 已通；新增接口会牵连 M4 的替换计划，不在本 spike 授权内 |

**本次止步于 1′**（DR-M3-3 的排序原则：按「是否要改 renderer」排序，首个可行即止）。

---

## 7. 产出与清理（不留一次性脚本）

保留（可回归，无 GUI，可进 CI）：

- `apps/desktop-tauri/test/portTransport.test.mts` —— 用真 `ChannelClient` 与真 `ChannelServer`
  在 stdio 帧上做真实 Initialize + promise 往返 + 未知命令的错误回传。
- `apps/desktop-tauri/test/fixtures/stdioChannelHost.ts` —— 上面的 Node 侧 Host 替身。
- 运行：`pnpm --filter @mode/desktop-tauri test`（已加进该包 `package.json` 的 `scripts.test`）。
  实测输出：

```text
✔ 真实 ChannelServer 的 Initialize 与一次 promise 往返可以走 stdio 帧 (287.4ms)
✔ 未知频道/命令的失败会作为 RPC 错误回到调用方，而不是挂住 (188.2ms)
ℹ tests 2   ℹ pass 2   ℹ fail 0
```

已删除（一次性；重放配方见 §8）：

- `apps/desktop-tauri/experiments/port-transport/**`（spike 页、注入桥、Vite 配置、Host 替身）
- `apps/desktop-tauri/src-tauri/src/bin/port_experiment.rs` 与 `Cargo.toml` 里的
  `[[bin]] PortTransportExperiment`（**Cargo.toml 已还原**，产品壳未留改动）
- `apps/desktop-tauri/src-tauri/capabilities/port-experiment.json`（§4.2 证明本地内容不需要它）
- `apps/desktop-tauri/dist/experiments/**`（构建产物）

本任务触碰的文件只有：`docs/plans/2026-10-16-m3-port-transport-spike.md`、
`apps/desktop-tauri/test/**`、`apps/desktop-tauri/package.json`。
`packages/rpc`、`packages/shared`、`packages/client`、`packages/services`、
`packages/desktop` **零改动**。

验证：`pnpm typecheck` 通过；`pnpm lint` 0 error（83 条 warning 全部在改动面之外）；
`node scripts/check-language-policy.mjs` 通过；`node scripts/rust-cargo.ts build --release -p mode-tauri` 通过。

## 8. 重放配方（约 60 行，Task 1 可直接改成正式实现）

```rust
// 1) Rust：窗口必须在 Rust 侧建，才挂得上 init 脚本（WindowConfig 没有该字段）
let script = include_str!(".../injected-bridge.ts").replace("__SPIKE_STARTUP_ID__", STARTUP_ID);
tauri::WebviewWindowBuilder::new(app, "port-experiment",
        tauri::WebviewUrl::App(std::path::PathBuf::from("experiments/port-transport/index.html")))
    .initialization_script(script)
    .build()?;

// 2) 页面 → 壳：整包 raw 字节
#[tauri::command]
fn spike_host_send(handle: State<'_, HostHandle>, request: tauri::ipc::Request<'_>) -> Result<(), String> {
    let bytes = match request.body() {
        tauri::ipc::InvokeBody::Raw(bytes) => bytes.clone(),
        other => return Err(format!("期望 raw body，实收 {other:?}")),
    };
    write_frame(&mut handle.0.stdin.lock().unwrap(), &bytes).map_err(|e| e.to_string())
}

// 3) 壳 → 页面：长轮询，返回 raw（页面侧拿到 ArrayBuffer）
#[tauri::command]
async fn spike_host_recv(handle: State<'_, HostHandle>) -> Result<tauri::ipc::Response, String> {
    let host = handle.0.clone();
    tauri::async_runtime::spawn_blocking(move || read_frame(&mut host.stdout.lock().unwrap()))
        .await.map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
        .map(tauri::ipc::Response::new)
}
```

```js
// 4) 注入桥（纯 JS，被当文本注入）：端口只能在页面内建
var channel = new MessageChannel();
var rendererPort = channel.port1, shellPort = channel.port2;

shellPort.addEventListener("message", function (event) {
  window.__TAURI_INTERNALS__.invoke("spike_host_send", event.data);  // Uint8Array 整包 → octet-stream
});
shellPort.start();
(function pump() {
  window.__TAURI_INTERNALS__.invoke("spike_host_recv").then(function (buffer) {
    if (buffer && buffer.byteLength > 0) shellPort.postMessage(new Uint8Array(buffer)); // 必须包 Uint8Array
    setTimeout(pump, 0);
  });
})();

// 5) 在 DOMContentLoaded（deferred module 脚本之后）投递：与 Electron preload 逐字同形
function deliver() {
  window.postMessage({ type: "mode:service-port", databaseStartupId: STARTUP_ID }, "*", [rendererPort]);
  window.postMessage({ type: "mode:database-startup-state", state: { /* ...phase: "ready" */ } }, "*");
}
```

---

## 9. 纪律与取证边界（clean-room 自证）

上游衍生的判定口径 = 文件在 `upstream/zcode-baseline`（实测
`git rev-parse upstream/zcode-baseline` = `aac4755666d09fdcd70272fcf063c077a639015f`）里存在。

本 spike **读过**的上游衍生文件与范围（只用于取协议面/签名，**未抄写任何实现体**）：

| 文件 | 读的范围 | 用途 |
| --- | --- | --- |
| `packages/rpc/src/protocol.ts` | 全文（任务书指定） | `MessagePortLike` / `MessagePortProtocol` 的声明与收帧判定 |
| `packages/rpc/src/channelServer.ts` | 全文（任务书指定） | `ChannelServer(protocol, ctx, timeoutDelay, deferInit)` 与 `ready()` |
| `packages/rpc/src/channelClient.ts`、`channels.shared.ts` | 导出签名 / 类型声明 | `getChannel().call()`、`onDidInitialize`、`IServerChannel` |
| `packages/client/src/messageport.ts` | 全文（59 行，任务书指定） | `connectViaMessagePort(port)` 的签名与组合关系 |
| `packages/desktop/src/renderer/src/main.tsx` | **两处窗口**（140–175、240–325） | 仅取两条消息的协议形状与 `event.source === window` 判定 |
| `packages/desktop/src/renderer/src/databaseStartupAdmission.ts` | 全文（31 行） | 准入闸门的接口（`acceptState/acceptPort/takeReadyPort` 语义） |
| `packages/shared/src/{channels,database-startup}.ts` | 全文 | 频道常量与状态 schema（协议形状即本任务要求） |

其余只读接口面的上游文件（`packages/desktop/src/preload/index.ts`、`host/**`）仅用于**核对既有结论**
（preload 用 `window.postMessage(..., [port])` 转交端口），未取实现细节。

`apps/desktop-tauri/**` 是 clean-room 区：本任务在该目录下新增的一切（spike 页、注入桥、实验 bin、
测试面）**全部是新写的**，未参考任何上游实现；实验代码已按 §7 删除，只留测试面。
