# 局域网连接挂接（LAN attach）握手时序

状态：能力位增补于 2026-10-11（见 [lan-remote-computer-use.md](./lan-remote-computer-use.md)）。

## 产品规则

- 局域网远程连接的挂接链路（`packages/desktop/src/host/lanRemoteAttach.ts`）：HTTP（`/api/server-info` 携带长期令牌 → `POST /api/rpc-host-capability` 取一次性 capability）→ 携 capability 打开 `ws://…/ws/host` → `SocketProtocol` → `ChannelClient` → `RemoteServiceAccess`，复用与 SSH 相同的 RPC 通道语义。
- `/api/server-info` 的 `capabilities` 是设备级能力协商面：`computerUse?: boolean` 表达对端机器是否具备电脑控制条件（win/mac + env kill-switch + desktop-local authority）。optional 缺键 = 不支持，旧对端握手必须成功；该位经 `HostRemoteConnectionCapabilities` → attachment 注册参数下发 renderer。
- **Initialize 不变量（本 spec 的核心）**：RPC 服务端（`ChannelServer`）只在建连瞬间下发**一帧** `Initialize`；客户端 `ChannelClient` 收到它之前处于 `Uninitialized`，所有请求进 `whenInitialized()` 静默排队。因此客户端的 WS `message` 监听**必须在等待 `open` 事件之前挂载**——Node `ws` 会在同一回调里先 emit `open`、紧接着处理握手回包中携带的 `Initialize`，若监听晚挂（`await open` 之后再 wrap），该帧被静默丢弃且**不会重发**。
- 事件顺序（客户端必须遵守）：

```text
new WebSocket(url)
  → 立即 wrap（挂 message 监听）+ 构造 SocketProtocol/ChannelClient
  → await open（失败则抛「连接对端通道失败」）
  → 此后任意 RPC：Initialize 已达 → 请求立即上线路由到对端
```

- 失败语义：丢掉 `Initialize` 后**没有报错、没有日志**，`systemService.info()` / `fileService.readdir()` 等全部永久 pending——表现为连接成功但目录选择步骤永远「加载中…」。任何改动不得回到「先等 open 再挂监听」的顺序。
- **关闭语义（2026-10-20 修复）**：WS `close` / `error` 必须**恰好一次**经 `attachLanRemoteConnection` 的 `onClose` 回调上抛；Host 的 `defaultConnectLan` 必须把它接到返回 handle 的 `onDidClose` 监听者。registry 收到后把 logical session 标记为 `disconnected` 并发出 `closed`，renderer 注销该 session、释放 scoped `ChannelClient`，挂起 RPC 以 `ConnectionClosed` reject。缺少这条接线时，renderer 会一直持有死连接，`modelSelectionService.getView()` 等请求永久 pending（表现为「正在加载模型…」且无法发送）。
- **存活检测**：LAN WS 建立后由客户端发 `ping`，未在下一个周期前收到 `pong` 则 `terminate()`，走同一条 `close` 关闭语义。该检测只判定传输存活，不给业务 RPC 增加超时；不允许用请求级超时掩盖真实同步问题。
- 不含改名前版本的兼容（用户已拍板不做）：cookie 名、凭据密钥的旧版兼容不在本 spec 范围。

## 验收场景

1. 回归测试 `packages/desktop/tests/lan-attach-initialize.test.mjs`（CI `node --test packages/desktop/tests/*.test.mjs` 覆盖）：拉起真实 `createHttpServer`（含 capability 与 `/ws/host`），调用真实 `attachLanRemoteConnection`，断言首条 RPC（`systemService.info()`）在超时内返回——修复前必须红（Initialize 被丢、请求悬空），修复后绿。
2. `/api/server-info` 返回 `capabilities.computerUse`（新对端为 boolean）；schema 校验对缺键（旧对端）通过且 UI 判为不支持。
3. 回归测试 `packages/desktop/tests/remote-workspace-host-service.test.mts`：走 `defaultConnectLan` 的默认路径（只注入 fake WS 挂接），触发 `onClose` 后必须收到 `closed` 事件；修复前 `onClose` 未接线，用例必须红。
4. 回归测试 `packages/desktop/tests/lan-access-server.test.mts`：端口被占用时 `startLanAccessServer` 必须以 rejection 返回（`局域网端口 … 监听失败`），不得作为未捕获 `error` 事件杀死 Host；同一用例覆盖端口 0 的正常监听与 dispose。
5. `pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed` 全绿。

## 本轮验证状态

- 已验证：最小复现两组对照（监听晚挂 → RPC 5s 超时；监听提前 → 2ms 返回）。
- **待实机**：两端真实实例各升到含本修复的构建后，走完「连接 → 选择目录 → 打开工作区」。
