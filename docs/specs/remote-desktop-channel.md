# Spec：`remote-desktop` 频道（Rust 首切，LT-16.4b）

状态：2026-11-23 首切落地。覆盖**不依赖媒体/信令链路**的部分：被控设置面、
逐设备授权、编码能力缓存、对端能力探测。会话面（startSession/sendSignal/
endSession/respondConsent/setLocalDisplays + 两个动态事件）经端口注入，未装配时
显式报错。

## 背景

`IRemoteDesktopService`（`packages/services/src/remote-desktop/remoteDesktopService.ts`）
是远程桌面的唯一 RPC 面，15 个成员（12 方法 + 3 事件）。Node 侧实现分两层：

- `packages/desktop/src/host-standalone/remoteDesktopService.ts`（535 行）：频道适配层，
  把设置命令翻成 server 消息；
- `packages/desktop/src/host/remoteDesktopServer.ts`：被控 server（唯一写者）。

## 1. 状态所有者

| 状态 | 所有者 | 落点 |
| --- | --- | --- |
| 被控总开关 | 凭据键 `remote-desktop:server-enabled`（值 `"1"` = 开） | `CredentialStore` |
| 逐设备授权 | `lan:access:clients` 记录的 `remoteDesktopAuthorized` 字段 | 同 LAN 客户端注册表 |
| 编码能力缓存 | `{appConfigDir}/rd-encoder-capability.json` 的 `capability` | 文件（跨进程保留观测值） |
| 被控会话（同意/接管） | Host 进程内的 server + 主控 controller | **端口注入**（本切不搬） |

三条不变量：

1. 授权事实**只有一处**：`lan:access:clients` 记录上的字段，与同意窗「记住此设备」同源；
   设置页的授权开关写同一字段，不另建授权表。
2. 编码能力**只由真实会话观测写入**，不猜、不预探测；读取严格白名单，坏文件按「无缓存」。
3. 被控会话态非法组合（同时 pending consent 与 active session）由端口自持，频道不复制。

## 2. 方法映射

位置参数按 `ProxyChannel` 约定（`arg` 是数组，对象在首位）。

| 方法 | 语义 | 本切 |
| --- | --- | --- |
| `getServerState()` | 读开关 + 授权列表 + 编码能力 | ✅ |
| `setServerEnabled({enabled})` | 写开关；关闭时需终止会话（端口） | ✅ 写盘；终止经端口 |
| `authorizeClient({clientId})` | 置 `remoteDesktopAuthorized=true` | ✅ |
| `revokeClient({clientId})` | 删该字段（不是置 false） | ✅ |
| `getPeerCapability({serverId})` | 探对端 server-info 能力位；失败归 offline、**不抛** | ✅ |
| `getControlledState()` | 被控态 | 端口；未装配 → `{controlled:false}` |
| `onDidChangeControlledState` | 幂等事件，首个订阅者补当前值 | 端口；未装配 → 只登记 |
| `startSession` / `sendSignal` / `endSession` / `respondConsent` / `setLocalDisplays` | 主控会话 | 端口；未装配 → 显式报错 |
| `onDynamicSessionEvent` / `onConsentRequest` | 会话事件 | 端口；未装配 → 只登记 |

## 3. 与 TS 的差异（登记）

1. **`readClients` 的「活 handle 优先」不适用**：TS 在 LAN server 起了监听时从内存数组读
   （避免与唯一写者抢），Rust 首切没有常驻 LAN server 句柄，一律读凭据。凭据是持久事实，
   语义等价；LAN supervisor 翻转后若要接内存写者，走同一个 broker 记录，不新建第二份。
2. **`setServerEnabled(false)` 的会话终止经端口**：TS 在 `remoteDesktopServer` 内同步
   `closeActiveSocket`。Rust 首切把「关开关 → 终止会话」交给端口实现；端口未装配时只落盘，
   并留一条 warn 日志（不静默假装已终止）。
3. **`getPeerCapability` 不做 WebRTC/中继选路**：只听 `server-info` 的能力位与端点可达性
   （公网优先、LAN 回退，与 `lan_peer_client` 同口径）。数据面选路在 16.4b 续。

## 4. 装配（**本切不翻转**）

- 频道构造需要 `data_root`（凭据 + 编码能力缓存路径）与可选的会话端口。
- **刻意不进 `service_channel_names()`，也不进 inprocess 默认集**。原因：壳内频道是
  **整频道切换**（AGENTS.md 与 `host-ownership-switch.md` 的硬约束），而 `remote-desktop`
  的会话面（startSession/sendSignal/endSession/respondConsent/两个事件）还没有 Rust 实现。
  现在翻面会让「设置页开关能用、但远程桌面连不上」——比不翻更糟。
- 翻转前置（缺一不可）：
  1. `RemoteDesktopPort` 的生产实现（被控 server + 主控 controller + 媒体/信令链路，
     复用 `crates/mode-cua-ax/src/rd_media` 与 `crates/remote-desktop-streaming`）；
  2. `onDidChangeServerState` 的发射点（本切显式报错，端口落地后再接）；
  3. 真机双机验收（设置页开关/授权、对端能力位、会话建立）。

## 5. 验收

- 频道契约（`crates/host-channels/tests/remote_desktop_channel.rs`，8 条）：
  开关读写往返（关 = 删键）、授权/撤销写同一记录字段（撤销删字段不置 false）、
  未知 clientId 幂等、编码能力白名单与坏文件降级为 `unknown`、
  `getPeerCapability` 未配对返回 offline 而不抛、
  会话方法未装配端口时**显式报错而不是返回假成功**、未知方法拒绝。
- 单元测试（`crates/host-channels/src/remote_desktop.rs`，4 条）：同上语义在模块内直测。
- `cargo clippy -p host-channels --all-targets -- -D warnings`。
- 真机（待用户，翻转后）：设置页开关/授权列表、对端能力位显示。
