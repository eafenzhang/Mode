# 局域网远端工作区的电脑控制（LAN attach 完整闭环）

状态：已拍板（2026-10-11）。关联：[computer-use-enablement.md](./computer-use-enablement.md)（启用权威与门控矩阵，本文只管 LAN 远端形态）、[lan-remote-attach.md](./lan-remote-attach.md)（握手时序，本文给握手补能力位）、[computer-use-windows-runtime.md](./computer-use-windows-runtime.md)（runtime/Helper 本体）。

## 背景与问题

电脑控制（CUA）此前在产品上明确划为「本机专属」：远端 workspace 一律显示「当前环境暂不支持」，输入框入口与权限事件面也在多处按 remote 关闭。需求为：**局域网（LAN attach）工作区对应的设备开启电脑控制后，本地针对该远端工作区的对话支持完整电脑控制闭环**（发起 + 展示 + 对话内权限授权）。

LAN 形态与 ssh/wsl/docker 有本质区别：LAN 对端是**已在运行的完整 Mode desktop 自己**（零部署，见 `lanRemoteAttach.ts` 注释），CUA 插件 seed、Helper、权限服务都天然存在于对端机器。当前阻断全部是「按 remote 一刀切」的门控残留，而非能力缺失：

1. 对端 helper 创建门把 `remote:lan:` identity 一并挡掉（`shouldCreateDefaultCuaProductHelper` 的 `!hasRemoteWorkspaceIdentity`），导致对端 spawn 不注入 broker、CUA 工具面起不来。
2. 权限服务 `ICuaPermissionService` 未注册进远端服务集合，本地 remote pane 拿到的是悬空代理（调用排队→超时）。
3. OS 权限观察事件被两道门拦：`modeAgentService` 对 remote identity 直接丢弃、`modeAgentConnectionScope` 的 role 门只放 `terminal-client`。
4. UI 四处硬编码远端不支持（availability / composer entry / 设置页 unsupported 卡 / 插件页）。
5. 没有「对端支持电脑控制」的能力位，UI 只能靠 identity 启发式反推。

## 范围与非目标

- **范围内**：LAN attach（identity `remote:lan:…`）的远端工作区；本地 desktop renderer 的 remote pane。
- **非目标**：ssh/wsl/docker 远程 workspace（对端无 desktop/Helper，条件不成立，维持不支持）；手机 Web 远控（`web-remote-replayable`）与 IM Bot 对话（维持不订阅权限事件面）；Web 设置分区平台门；Linux 对端。

## 产品规则

1. **支持判定 = 设备能力位 AND 对端插件启用态 AND 共用门控**：
   - 设备能力位 `computerUse`（握手一次性，见接口变更）表达「对端机器能不能」：平台 ∈ {win32, darwin}、env kill-switch 开、服务方为 desktop-local authority。
   - 插件启用态仍是 `enabledPlugins["computer-use@mode-plugins-official"]`，唯一状态、唯一写路径不变（插件管理 RPC 已透传到对端，本地设置页拨开关写的即对端配置）。
   - env kill-switch 与插件启用态的 AND 结构（`resolveCuaWorkspaceEnablement`）不改。
2. **完整闭环三层**：
   - **发起**：remote pane 输入框显示「电脑操作」入口，发起的 turn 在**对端机器**上 spawn、注入 broker、Helper 操作对端物理屏幕。
   - **展示**：对话内 CUA 工具卡片、截图帧、`request_access` 结果卡片随对话帧渲染（该链路本无 remote 门）；模型侧权限同意/拒绝（`resolveInteraction` 回执）同样无 remote 门，直接可用。
   - **权限**：本地对话可查看对端 OS 权限状态（`getStatus` 经透传服务）、可远程触发**对端机器**的权限引导窗口；OS 级弹窗（mac TCC 等）由人在对端设备上完成，属一次性前置条件。
3. **旧对端降级**：`server-info` 缺 `computerUse` 键（旧版对端）视为不支持，UI 维持现有「暂不支持」卡片；不得因未知键导致握手失败（zod optional）。

## 状态所有者

| 状态 | 唯一所有者 | 写路径 |
|---|---|---|
| 插件启用态 | 对端机器的 workspace 配置投影 | 既有 `pluginManagementStore.setEnabled` → 透传 pluginManagementService |
| 设备能力位 | 对端 LAN access server（`createServerInfo`） | 握手时计算，无持久化 |
| 能力位在本地的投影 | `HostRemoteConnectionCapabilities` → attachment 注册参数 → `RemoteWorkspaceSession` | 连接生命周期内只读 |
| Helper / broker 凭据 | 对端 host 进程（desktop-local authority） | 既有 enablement admission |
| OS 权限状态 | 对端 `cuaPermissionService` | 对端 Helper/系统查询；本地只读 + 触发引导 |
| 会话与 turn 状态 | 对端 host（不变） | 本地仅发起命令与观察 |

## 事件顺序

```text
① 握手（一次性）
本地 desktop                      对端 desktop（LAN peer）
  lanRemoteAttach ──GET /api/server-info──► createServerInfo
    ◄── capabilities.computerUse ──────────┘  （平台 && kill-switch && desktop-local）
  HostRemoteConnectionCapabilities.computerUse 填充
  → attachment 注册参数 → RemoteWorkspaceSession.capabilities（renderer）
  → UI 四门判定（capability && 插件启用态 && 本机平台门）

② 发起 turn
renderer composer 入口 → V4 conversation command
  → 对端 modeAgentService spawn（identity=remote:lan:…）
  → 对端 enablement admission：desktop-local && lan identity → 放行
  → 注入 broker env → CUA MCP → Helper → 对端物理屏幕

③ 展示与对话内授权
对端 CLI 事件 → V4 conversation 帧 → LAN 回传 → 本地对话渲染
  工具卡片 / 截图帧 / request_access 结果 / 权限弹窗（pendingInteractions）
  用户点同意 → resolveInteraction 命令 → 对端 interaction-registry（幂等先到先得）

④ OS 权限观察与引导
对端 CLI request_access 缺权限 → v4/cua/permission-observation
  → 对端 modeAgentService：lan identity 放行（ssh/wsl/docker 继续丢）
  → scope：trusted-host-relay && 下游 desktop-continuous 放行（手机 replay 下游仍关）
  → 本地 CuaPermissionObservationAttachment 显示状态卡
  → 「去授权」→ ICuaPermissionService.openPermissionOnboarding()（透传）
  → 对端主进程打开对端机器的引导窗口（绝不打开本机的）
  → 人在对端完成 OS 弹窗 → getStatus 复查 → restartHelper 生效
```

## 接口变更

1. **握手能力位**：`serverRemoteInfoSchema.capabilities` 新增 `computerUse?: z.boolean()`（optional，缺键=不支持）。由 `createServerInfo` 按设备条件填充；`LanRemoteConnection.info` 的 capabilities 收口进 `HostRemoteConnectionCapabilities`（新增 `computerUse?: boolean` 字段）。
2. **下发 renderer**：LAN handle 的 attachment 注册参数携带该位 → `RemoteWorkspaceSession` 新增只读 `capabilities?: { computerUse?: boolean }`；UI 判定函数（`resolveComputerUseAvailability`、`resolveCuaComposerEntryView`）输入面新增该位。
3. **权限服务透传**：`createRemoteWorkspaceServiceCollection` 注册 `ICuaPermissionService`（转发对端），并加入 `legacyRemoteWorkspaceRpcContract` 必选白名单；同时修 `RemoteServiceAccess` 对该 descriptor 的无条件构造（按 Initialize 通道列表判定存在性，消除悬空代理）。
4. **权限引导远程触发**：`ICuaPermissionService` 新增 `openPermissionOnboarding(options?)`；对端实现经 host→main 打开**对端机器**的 onboarding。本地 pane 的「去授权」动作改走该方法；本机 pane 维持既有 `platform.openCuaPermissionOnboarding`。
5. **事件面放行**（见事件顺序 ④）：`modeAgentService` 丢弃条件收窄为「remote identity 但 kind 非 lan 才丢」；`modeAgentConnectionScope.onDynamicCuaPermissionObservation` 照 `onDynamicConversationTelemetryFact` 的 relay 先例放行 `trusted-host-relay && 下游 desktop-continuous`。
6. **对端 enablement 放行**：`shouldCreateDefaultCuaProductHelper` 的 identity 条件收窄为「无 remote identity，或 identity kind 为 lan」（authority 条件不变，仍要求 `desktop-local`；ssh/wsl/docker identity 继续排除）；spawn broker 注入与动态 resolver 沿用同一判定，保持同源。

## UI 规则

- `resolveComputerUseAvailability` 新增 kind `remote-lan`：能力位为真 → `supported: true`；否则维持 `supported: false`（文案不变）。ssh/wsl/docker/Linux/Web 分支不变。
- 输入框「电脑操作」入口：remote pane 在能力位为真 + 插件启用 + 本机平台门（win/mac renderer）满足时可见；门结构（平台/设置/服务/插件四层）不变，只改输入。
- 设置页「电脑控制」分区与插件页：LAN pane 能力位为真时显示正常开关（写透传到对端），不再进 unsupported 卡；能力位缺失时文案维持现状。
- i18n：新增「对端设备」语境文案（如 unsupported 原因、权限引导按钮），中英同步；不改既有 key 的语义。

## 降级与失败语义

- 旧对端无能力键 → 全链路按现状不支持；握手不报错。
- 对端 kill-switch 关或非 win/mac → 能力位 false，同上。
- 对端 Helper 启动失败 → 对端 agent 错误经既有对话错误面暴露；不加超时兜底、不加本地兜底分支。
- 权限服务调用失败 → `useCuaPermissionStatus` 现有容错（快照恒空）保留；透传注册后不再出现「排队→Unknown channel 超时」。
- 插件未启用 → 与本地一致：入口/分区按既有插件层引导处理。

## 验收场景

- [ ] 旧版对端（无 `computerUse` 键）握手成功，本地 pane 与设置页维持「暂不支持」。
- [ ] 新对端 win/mac + kill-switch 开：能力位为真；对端插件未启用时设置页可直接拨开（写对端配置），输入框入口随后出现。
- [ ] 经入口发起 turn：对端 spawn 注入 broker、Helper 操作对端物理屏幕；Windows 对端顶部出现操作指示（投影在对端）。
- [ ] 对话内 CUA 工具卡片与截图帧在本地 remote pane 正常渲染；权限弹窗同意/拒收回执生效（幂等）。
- [ ] 对端缺 OS 权限时：本地对话出现权限观察卡片；点「去授权」后**对端机器**弹出引导窗口，本机不弹。
- [ ] ssh/wsl/docker workspace 与手机远控行为不变：入口不可见、unsupported 卡片、权限观察事件不投递。
- [ ] 单测：能力位 schema 缺键兼容；enablement lan identity 放行/ssh 排除；scope 事件面（relay desktop 下游放行、replay 下游拒绝）；availability `remote-lan` 分支；`lan-attach-initialize` 回归不破。
- [ ] `pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed` 全绿。
