# Windows 远程桌面（LAN）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在「插件市场」菜单下方新增「远程桌面」入口，Windows ↔ Windows 的 Mode desktop 之间通过局域网提供实时屏幕画面与键鼠控制（spec 的 P1）。

**Architecture:** 复用既有 LAN 信任链（发现/配对令牌/server-info 能力位/一次性票据），媒体走 WebRTC P2P（Electron/Chromium 自带，无新增第三方依赖），被控端用隐藏渲染器采集与编码，输入注入复用既有 CUA Helper（新增 `raw_input` broker 方法，不走 CUA 动作语义）。信令经被控端宿主（lanAccessServer）在两端 main 之间转发。

**Tech Stack:** TypeScript（packages/shared、packages/desktop、packages/ui、packages/server、packages/services）+ Rust 1.99（crates/mode-cua-ax，napi-rs）+ Electron 41（WebRTC/desktopCapturer/getDisplayMedia）。

**Spec:** `docs/specs/lan-remote-desktop.md`（已拍板 2026-10-12）；spike 数据与两机清单：`docs/plans/2026-10-12-remote-desktop-spike.md`。

## Global Constraints

- 仅 win32 对端；UI 入口 `isWindowsDesktop` 门；不引入 macOS/Linux/公网中继路径。
- 新代码只允许 TS/Rust/HTML/CSS（`docs/specs/language-policy.md`）；不许新增 `.mjs/.js`（spike harness 保持在 `.spike/` 未跟踪区，不入库）。
- 无新增第三方依赖：媒体用 Electron/Chromium 自带 WebRTC；`--disable-features=WebRtcHideLocalIpsWithMdns`（spike 已验证候选为裸 IP）。
- 信任链唯一：既有配对令牌 + 一次性会话票据（30s、单次消费、绑定 clientId）；不新增第二套配对。
- 被控默认关闭 + 逐设备授权 + 会话中常驻可见指示 + 本地随时终止；kill-switch env（只能关不能开）。
- 注入唯一写路径 = CUA Helper（`raw_input`）；renderer 不触达 Helper 与令牌；main 不落业务 session 状态之外的状态。
- 每任务：先测后写；`pnpm typecheck`、`pnpm lint`、受影响包测试全绿才提交；提交只 add 本任务路径（pathspec 纪律），不 push。
- UI 经 `packages/ui/src/hooks/` 访问服务；平台操作经 `IPlatformService`；i18n 中英同步；日志遵循 `AGENTS.md` 日志节。

---

### Task 1: 共享协议面（types + 纯函数 + 能力位）

**Files:**
- Create: `packages/shared/src/remote-desktop.ts`
- Modify: `packages/shared/src/server-remote.ts:18-25`（capabilities 增 `remoteDesktop`)
- Modify: `packages/shared/package.json`（exports 增 `"./remote-desktop"`）
- Test: `packages/shared/test/remote-desktop.test.ts`

**Interfaces（后续任务全部依赖，命名不可漂移）:**
- `REMOTE_DESKTOP_PROTOCOL_VERSION = 1`
- `remoteDesktopMonitorSchema` / `RemoteDesktopMonitor`：`{ displayId: string; label: string; widthPx: number; heightPx: number; scaleFactor: number; primary: boolean }`
- `remoteDesktopSignalMessageSchema` / `RemoteDesktopSignalMessage`（zod discriminated union，`type`）：
  `hello{protocolVersion}`、`monitors{monitors[]}`、`select-monitor{displayId}`、`offer{sdp}`、`answer{sdp}`、`ice{candidate, sdpMid, sdpMLineIndex}`、`bye{reason?}`、`error{code, message?}`
- `RemoteDesktopErrorCode = "consent_denied" | "consent_timeout" | "disabled" | "busy" | "unsupported" | "unauthorized" | "stale_ticket" | "internal"`
- `remoteDesktopPointerEventSchema` / `RemoteDesktopPointerEvent`：`{ t: "pm"; x: number; y: number }`（x/y ∈ [0,1]，相对所选显示器画面）
- `remoteDesktopControlEventSchema` / `RemoteDesktopControlEvent`：`{t:"md"|"mu"; button: RemoteDesktopMouseButton; x; y}`、`{t:"mw"; direction:"up"|"down"|"left"|"right"; delta:number; x; y}`、`{t:"kd"|"ku"; vk:number}`、`{t:"tx"; text:string}`
- `RemoteDesktopMouseButton = "left" | "right" | "middle"`
- 纯函数：
  - `computeNormalizedPointFromVideoEvent(videoRect:{left,top,width,height}, intrinsic:{width,height}, client:{x,y}): {x,y,inside:boolean}` — object-fit:contain letterbox 换算，clamp 到 [0,1]，`inside` 标记是否落在画面内容内。
  - `describeKeyAction(ev:{key,keyCode,ctrlKey,altKey,metaKey}): {kind:"text",text:string}|{kind:"vk",vk:number}|{kind:"none"}` — 规则：Ctrl/Alt/Meta 按住或 `key.length!==1`（修饰键与导航键）→ vk(keyCode)；可打印字符 → text。
  - `mapMonitorNormalizedToVirtualAbsolute(p:{x,y}, monitor:{xPx,yPx,widthPx,heightPx}, virtual:{xPx,yPx,widthPx,heightPx}): {x:number,y:number}` — 输出 0..65535 的 MOUSEEVENTF_VIRTUALDESK 归一化坐标（含 clamp）。

**Steps:**
- [ ] 1. 写失败测试（node:test + assert，覆盖：信号消息 union 判别与拒绝非法 type；`hello.protocolVersion` 为字面量 1；pointer/control 事件字段校验；三纯函数的边界——letterbox 上/左外边 → inside:false 且 clamp；vk 规则表含 Ctrl+C、方向键、单独 Shift；虚拟屏映射含多屏负原点与 clamp）。
- [ ] 2. `cd packages/shared && pnpm test` → 预期失败（模块不存在）。
- [ ] 3. 实现 `remote-desktop.ts`；`server-remote.ts` capabilities 增 `remoteDesktop: z.boolean().optional()`；`package.json` exports 增条目。
- [ ] 4. `cd packages/shared && pnpm test` → 全绿（含既有 `server-remote-capabilities.test.ts` 不回归）。
- [ ] 5. 提交：`git add packages/shared/src/remote-desktop.ts packages/shared/src/server-remote.ts packages/shared/package.json packages/shared/test/remote-desktop.test.ts && git commit -m "feat(shared): 远程桌面协议类型、坐标映射与能力位"`

### Task 2: 被控端 host 信令与票据

**Files:**
- Modify: `packages/server/src/http.ts`（createHttpServer 选项增 `remoteDesktop?: { requestSession, consumeTicket, onSessionSocket }`；新增 `POST /api/remote-desktop/session`、`GET /ws/remote-desktop?ticket=`）
- Modify: `packages/desktop/src/host/lanAccessServer.ts`（实现 hooks：requestSession 经 main 同意流 → 票据；onSessionSocket 转发给 main）
- Test: `packages/server/test/`（按既有 server 测试形态）+ `packages/desktop/tests/`（mjs 形态，按既有命名）

**Interfaces:**
- HTTP：`POST /api/remote-desktop/session`（Bearer 令牌，与 `/api/*` 同鉴权）→ `200 {ticket}` / `4xx {code: RemoteDesktopErrorCode}`；`GET /ws/remote-desktop?ticket=<t>` 升级（单次消费、30s、绑定 clientId；失败 401 `stale_ticket`）。信令帧为 UTF-8 JSON 单帧（`RemoteDesktopSignalMessage`）。
- Consumes（T1）：`RemoteDesktopSignalMessage`、`RemoteDesktopErrorCode`。
- Produces：host 侧 hook 签名 `requestSession(clientId): Promise<{ticket:string}|{error:RemoteDesktopErrorCode}>`、`onSessionSocket(clientId, sock)`；能力位 `createServerInfo` 增 `remoteDesktop = process.platform==="win32" && <被控开关> && kill-switch`（开关读取点见 T6 produce 的 `remoteDesktopServerEnabled` 读取 API；T2 先接 kill-switch + 平台，开关读取由 T6 注入回调）。
- 票据实现复用 `packages/server/src/hostCapability.ts` 模式（新增独立 store，勿复用 host 票据本体）。

**Steps:**
- [ ] 1. 读 `server/src/http.ts` 既有 `/ws/host` capability 与 `/api/lan/pair` 实现，写 server 层单测（票据一次性/过期/错客户端 → 401；POST 未授权 → 401；能力位计算）。
- [ ] 2. 实现 http.ts 端点与选项注入；单测绿。
- [ ] 3. lanAccessServer 接线（requestSession 暂以"直接拒绝 disabled"占位，main 桥在 T3 接入；onSessionSocket 暂收 socket 并转发到 main 的空实现，T3 补全）。
- [ ] 4. `pnpm typecheck && pnpm lint`；跑 `packages/server` 与 `packages/desktop` 相关测试。
- [ ] 5. 提交（路径：`packages/server/src/http.ts packages/server/test/<新文件> packages/desktop/src/host/lanAccessServer.ts`）。

### Task 3: main↔host 桥 + 被控 main 会话协调器

**Files:**
- Create: `packages/desktop/src/main/remoteDesktop/`（`sessionCoordinator.ts`、`consentWindow.ts`、`indicatorWindow.ts`、`captureWindow.ts`、`settings.ts`）
- Modify: `packages/desktop/src/host/index.ts`（HostMessageTypes 增 `RemoteDesktop*` 消息）、`packages/desktop/src/main/desktopRemoteSessions.ts` 邻近新模块接线 main 侧消息路由
- Modify: `packages/desktop/src/main/desktopMainIpcPlatform.ts`（`rd:*` IPC）
- Test: `packages/desktop/tests/*.mjs`（状态机与消息转发，纯逻辑部分）

**Interfaces:**
- 被控会话状态机（main 唯一所有者）：`idle → requesting(设备) → waitingConsent → active → closed`；转移函数纯逻辑导出以便单测。
- `rd:*` IPC（renderer→main，T5 消费）：`rd/request-session{peerServerId}`、`rd/end-session{}`、`rd/session-state{}`、`rd/input{channel,payload}`（仅主控方向）、`rd/consent-result{accept,remember}`（同意窗口）。
- 同意读取 API（T2/T6 消费）：`isRemoteDesktopServerEnabled()`（settings）、`listAuthorizedClients()`、`authorizeClient(clientId,label)`。
- Consumes（T2）：host hook 注入点；`requestSession` 完成"同意流"：main 收到 host 的 `RemoteDesktopConsentRequest{clientId,label}` → 弹窗（60s 超时）→ 回 `RemoteDesktopConsentResult`；票据由 host 签发。
- 隐藏采集窗：`BrowserWindow{show:false, webPreferences:{backgroundThrottling:false, contextIsolation:true}}` 载 `remoteDesktopCapture.html`；`session.setDisplayMediaRequestHandler` 按窗口 webContents 匹配返回 `desktopCapturer` 屏幕源（spike 已验证）。
- 指示浮层/同意弹窗：沿用 `windowsCuaOperationIndicator` 窗口形态先例；i18n 文案 key 由 T6 提供。

**Steps:**
- [ ] 1. 状态机纯函数 + 单测（红→绿）。
- [ ] 2. 同意弹窗与指示浮层窗口（含"结束"按钮 → end-session）。
- [ ] 3. 隐藏采集窗 + display media handler + monitors 枚举（`desktopCapturer.getSources` ↔ `screen.getAllDisplays` 关联 displayId）。
- [ ] 4. host 桥接线：`requestSession` 走同意流；`onSessionSocket` 转发到 main；main 把 WS 信令桥接到隐藏渲染器（MessagePort 或 executeJavaScript 事件派发）。
- [ ] 5. `pnpm typecheck && pnpm lint`；desktop 测试套绿。
- [ ] 6. 提交。

### Task 4: Rust `raw_input`（crate + helper + broker + 准入）

**Files:**
- Modify: `crates/mode-cua-ax/src/lib.rs`、`input.rs`（新增 napi 导出 `raw_input(events: Vec<RawInputEvent>)` 或逐方法导出）+ `crates/mode-cua-ax/tests/`
- Modify: `packages/mode-cua/helper/server.mjs`（backend 增 `raw_input`）、`broker.js`（BROKER_METHODS 增）、`helper/addon.mjs`（包装导出）
- Modify: `packages/services/src/node.ts` + `cua-permission-broker/windowsCuaHelperHost*`（准入第二理由：被控开关开）与注入端口（host 可调 `raw_input`）
- Test: `cargo test -p mode-cua-ax`；helper mts/mjs 测试

**Interfaces:**
- broker 方法 `raw_input`：`params { events: RawInputEvent[] }` → `{ dispatched: n }`；`RawInputEvent`（JSON）：`{k:"move", x:number, y:number}`（0..65535 VIRTUALDESK 归一化绝对坐标）、`{k:"button", button:"left"|"right"|"middle", down:boolean, x, y}`、`{k:"wheel", direction:"up"|"down"|"left"|"right", amount:number, x, y}`、`{k:"key", vk:number, down:boolean}`、`{k:"text", text:string}`。
- 语义：**不走 UIA、无前台校验、无 ensure_foreground**（spike 已证现有 perform 不可复用）；全部经 SendInput；坐标用 `MOUSEEVENTF_ABSOLUTE|MOUSEEVENTF_VIRTUALDESK`（复用 `input.rs:543-553` 归一化）。
- Consumes（T1）：坐标已由调用方换算为 VIRTUALDESK 归一化（shared 纯函数），Rust 不再做监视器换算。
- Produces：`cuaHelper.rawInput(events)`（services 侧封装，T3 注入路径消费）。

**Steps:**
- [ ] 1. Rust：先写单元测试（坐标归一化边界、事件序列 dispatch 计数；注入真实性用现有 test 模式跑一次实机点击各自窗口——沿 `tests/perform.rs` 先例）。
- [ ] 2. 实现 napi 导出；`cargo test -p mode-cua-ax` 绿。
- [ ] 3. helper/broker 接线（server.mjs 校验 events 数组与字段枚举；broker.js 方法表）；`node scripts/build-cua-helper.mjs` 重建 dist 并核对 manifest。
- [ ] 4. services 接入：helper 准入条件与注入调用口（不改 spawn 拓扑）。
- [ ] 5. `pnpm typecheck && pnpm lint`；services 测试绿。
- [ ] 6. 提交。

### Task 5: 主控 renderer 会话面（WebRTC + 输入采集）

**Files:**
- Create: `packages/ui/src/remote-desktop/`（`RemoteDesktopSession.tsx`、`useRemoteDesktopSession.ts`、`InputCapture.ts`、`videoFit.ts`）
- Modify: `packages/ui/src/app-shell/WorkspaceShellLayout.tsx`、`WorkspaceSidebar.tsx`、`App.tsx`、`app-shell/types.ts`（`WorkspaceMainView` 增 `"remote-desktop"`）、`app-shell/useWorkspaceTaskNavigation.ts`
- Create: `packages/ui/src/remote-desktop/RemoteDesktopHome.tsx`（设备页，T6 接线）
- Test: `packages/ui/test/*`（坐标映射与输入决策复测 shared 纯函数；组件态最小断言）

**Interfaces:**
- 页面经 `packages/ui/src/hooks/` 新增 `useRemoteDesktop()`：`{ devices, sessionState, connect(serverId), disconnect(), sendInput(channel,event), selectMonitor(displayId), stats }`；底层调 `IPlatformService` 的 `rd*` 方法（T3 的 IPC 面）。
- 会话 WebRTC 接线（renderer）：`new RTCPeerConnection({iceServers:[]})`；主控为 offer 方或 answer 方由信令 `hello` 后定（实现取 answer 方：被控发起 offer）；`setCodecPreferences` 优先 H.264（VP8 兜底，spike 默认 VP8 已验证兼容）；DataChannel 两条：`control`（ordered）与 `pointer`（`maxRetransmits:0`）。
- 输入采集：视频元素 `pointerdown/move/up`、`wheel`、`keydown/keyup`；坐标用 T1 `computeNormalizedPointFromVideoEvent`；键盘用 T1 `describeKeyAction`（text→`{t:"tx"}`；vk→kd/ku）；`e.preventDefault()` 且 `tabIndex=0` + 自动聚焦；`contextmenu` 拦截。
- 画面：`<video>` object-fit contain，`cursor:none`（对端光标随帧）；工具条：显示器切换（`select-monitor`）、画质档（`contentHint: "detail"|"motion"`）、全屏、断开；状态条（fps/RTT 用 `getStats` 3s 轮询）。

**Steps:**
- [ ] 1. `types.ts` union + 侧边栏按钮 + 主区空壳分支（可跑通导航；`isWindowsDesktop` 门）。
- [ ] 2. `useRemoteDesktopSession` 信令+WebRTC 骨架（本地两窗对打可连，对照 spike harness 流程）。
- [ ] 3. 输入采集与映射接线（键盘规则、双通道）。
- [ ] 4. 会话面 UI（工具条/状态条/全屏/断开）；i18n key 中英。
- [ ] 5. `pnpm typecheck && pnpm lint`；ui 测试绿。
- [ ] 6. 提交。

### Task 6: 设备页、设置分区与被控开关

**Files:**
- Modify: `packages/ui/src/remote-desktop/RemoteDesktopHome.tsx`（设备卡片：`getLanPairedPeers()` + 逐设备拉 `server-info` 能力位；连接按钮 → `connect(serverId)`；空态引导跳设置·局域网访问）
- Modify: `packages/ui/src/settings/settingsPageConfig.ts`（新区 `remoteDesktop`，win32 门）、`packages/ui/src/settings/`（`RemoteDesktopSettingsSection.tsx`：被控总开关默认关、已授权设备列表可撤销）
- Modify: `packages/ui/src/i18n/locales/zh-CN.ts`、`en-US.ts`
- Modify: `packages/desktop/src/main/remoteDesktop/settings.ts`（开关持久化，唯一写路径；不吞并 `lanAccess` 设置）
- Test: `packages/ui/test/*`（可用性判定纯函数、开关读写）

**Steps:**
- [ ] 1. 开关存储 + 平台 API + 可用性判定纯函数 + 单测。
- [ ] 2. 设备页与空态；能力位展示（旧对端缺键 → 不支持）。
- [ ] 3. 设置分区 UI + 授权设备管理；i18n。
- [ ] 4. `pnpm typecheck && pnpm lint && pnpm architecture:check --changed`；测试绿。
- [ ] 5. 提交。

### Task 7: 集成验证与收口

**Files:**
- Modify: `docs/specs/lan-remote-desktop.md`（验收勾选、验证状态）
- Test: 全量矩阵 + 两机清单（`docs/plans/2026-10-12-remote-desktop-spike.md`）

**Steps:**
- [ ] 1. 全量：`pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed`、`pnpm --filter @mode/shared test`、受影响包测试。
- [ ] 2. 本机回环验收：被控开关开 → 双实例（dev-data-dir 隔离）或双机 → 连接/画面/键鼠/多屏/结束清理。
- [ ] 3. 两机清单执行（用户侧）；spec「本轮验证状态」按实填写（未跑项如实标注）。
- [ ] 4. 提交收口。

## 风险与回退

- T3 的 main↔host 桥是唯一集成密集点：先用最小消息通道（host `HostMessageTypes` 扩展）打通单条 `consent-request` → 再扩信令转发。
- WebRTC 在 DevTools 打开时帧率显示异常属已知 Chromium 行为，验收以无 DevTools 为准。
- 若 H.264 `setCodecPreferences` 在 Electron 41 不生效，回退 VP8（spike 已证可用），在 spec 备注。
