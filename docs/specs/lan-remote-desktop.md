# 局域网远程桌面（Windows 实时画面与键鼠控制）

状态：已拍板（2026-10-12）。实现计划：[docs/superpowers/plans/2026-10-12-lan-remote-desktop.md](../superpowers/plans/2026-10-12-lan-remote-desktop.md)；本机 spike 记录：[docs/plans/2026-10-12-remote-desktop-spike.md](../plans/2026-10-12-remote-desktop-spike.md)。关联：[lan-remote-attach.md](./lan-remote-attach.md)（LAN 配对、令牌与握手时序）、[lan-discovery.md](./lan-discovery.md)（设备发现）、[lan-paired-devices.md](./lan-paired-devices.md)（已配对设备管理）、[lan-remote-computer-use.md](./lan-remote-computer-use.md)（模型驱动的远端电脑控制，与本功能互补）、[computer-use-windows-runtime.md](./computer-use-windows-runtime.md)（Helper 运行时与打包）、[computer-use-enablement.md](./computer-use-enablement.md)（kill-switch 语义先例）。

## 背景与问题

已有两种「控制远程电脑」形态，都不是人肉实时远程桌面：LAN CUA 是对话内让本机 Agent 回合制操作对端物理屏（截图进对话）；手机 Web 远控控制的是会话（命令/快照），不是屏幕。

需求：侧边栏「插件市场」下方新增「远程桌面」菜单，两端均为 Windows 的 Mode desktop 之间提供类似 Windows 远程桌面的实时效果——对端屏幕实时可见、本机鼠标键盘直接操作对端，含多显示器选择、被控端同意与进行中可见指示。

可行性依据（现状）：
- 连接与信任已齐备：UDP 发现 + Tailscale 单播（`desktopLanDiscovery.ts`）、配对码 → 逐设备长期令牌（`lanAccessServer.ts`）、一次性 capability 票据（`packages/server/src/hostCapability.ts`），整体复用。
- 被控端采集与注入原语已存在：`crates/mode-cua-ax`（GDI 截屏、SendInput 键鼠、锁屏探测、虚拟屏坐标归一化），但形态是单帧/模型驱动，需新增原始事件注入面。
- 媒体链路为零基础：无 WebRTC/编码器依赖；现有 RPC/relay 通道是 JSON+base64、1 MiB/帧、`binaryFrames:false`，不承载视频。媒体走独立 WebRTC P2P 通道（Electron/Chromium 自带，无新增第三方依赖）。

## 范围与非目标

范围内：
- 两端均为 Windows（win32 + x64）的 Mode desktop，LAN 直连（含 Tailscale 地址）；
- 单主控单被控；实时画面（含系统光标）、鼠标（移动/按键/滚轮/拖拽）、键盘（键位 + Unicode 文本）；
- 多显示器枚举与切换；会话同意弹窗、进行中可见指示与随时终止。画质档位推迟至 P2（需先扩信令协议）。

非目标：
- macOS/Linux 对端；手机 Web/浏览器端；
- 公网中继/NAT 穿透（仅局域网与 Tailscale 直连）；音频；文件传输（工作区 attach 已覆盖文件面）；剪贴板同步（P2）；
- 多主控并发；自动重连；游戏级低延迟；
- 不引入 RustDesk 等第三方远控实现（AGPL 许可与 clean-room 边界，见 [clean-room-policy.md](./clean-room-policy.md) 精神）。

## 产品规则

1. **被控端显式启用**：设置页新增「远程桌面」分区，总开关默认关（win32 才显示）。总开关关 → 连接请求直接拒绝，不弹窗。
2. **唯一信任来源是既有 LAN 配对**：不新增第二套配对。未配对设备无法发起。首次连接由被控端弹「<设备名> 请求远程控制」对话框（60s 超时=拒绝），可选「记住此设备」写入逐设备授权；已记住的设备直接进入。
3. **会话中指示常驻**：被控端显示顶层浮层「<设备名> 正在远程控制本机」+「结束」按钮；本地用户随时可终止；锁屏/安全桌面无法采集时画面冻结并提示，解锁自动恢复。
4. **输入共享**：远程输入与本地键鼠共用同一物理会话（无接管互斥）；与 CUA 工具并存不做仲裁（P2 项）。
5. **能力位**：`server-info.capabilities.remoteDesktop` = win32 && 被控开关 && kill-switch（镜像 `computerUseCapability` 先例，握手即时计算、无持久化、缺键=不支持）。设备卡不支持/离线时入口置灰。
6. **坐标映射**：主控画面按视频实际尺寸 letterbox 适配；鼠标坐标以「所选显示器源分辨率」归一化传输，被控端换算为虚拟屏绝对坐标（复用 `input.rs` 既有 VIRTUALDESK 归一化先例）；混合 DPI 多屏按源分辨率独立处理。
7. **光标**：画面内显示对端光标（采集自带）；主控侧在视频面上隐藏本地光标，避免双光标。
8. **画质档位**：流畅优先 / 清晰优先（映射 `contentHint` 与 `degradationPreference`）；自适应码率交给 WebRTC。

## 状态所有者

| 状态 | 唯一所有者 | 写路径 |
| --- | --- | --- |
| 被控总开关 | 被控端设置（settings service） | 设置页「远程桌面」分区；默认关 |
| 逐设备授权（记住的设备） | 被控端设置 | 同意弹窗「记住」写入；设置页撤销 |
| 配对与设备令牌 | 既有 lanAccess 凭据服务（不变） | 既有配对流程 |
| 能力位 remoteDesktop | 被控 host（`createServerInfo`） | 握手即时计算，无持久化 |
| 会话票据 | 被控 host | 请求-同意后一次性签发（30s TTL、单次消费、绑定 clientId） |
| 信令通道 | 被控 host 仅转发 | 不解析 SDP/ICE、不持久化 |
| 被控会话状态机 | 被控 main | idle → 请求中 → 等待同意 → 进行中 → 结束（单向，失败即收口） |
| 主控会话状态机 | 主控 main（信令与生命周期）+ renderer（媒体/输入/UI） | renderer 面驱动，main 转发信令 |
| 屏幕采集 | 被控隐藏渲染器（main 创建/销毁） | `getDisplayMedia`（desktopCapturer 屏幕源） |
| 输入注入 | 既有 CUA Helper（唯一 SendInput 实现） | 新增 broker 方法 `raw_input`；main 经宿主转发 |

## 事件顺序

```text
① 设备与能力（复用既有发现/配对，无改动）
被控 host  createServerInfo.capabilities.remoteDesktop = win32 && 被控开关 && kill-switch
主控设备页  getLanPairedPeers() + discoverLanPeers()；能力位按需经 server-info 拉取（同
            getLanPeerWorkspaces 路径先例）

② 连接请求与同意（主控 main → 被控 host → 被控 main）
主控 main ──POST /api/remote-desktop/session（设备令牌）──► 被控 host
  被控 host：校验令牌 + 能力位；会话占用中 → busy 直接拒
  ──► 被控 main：弹同意（已记住则跳过；总开关关/拒绝/60s 超时 → 稳定错误码）
  ◄── 允许：host 一次性会话票据（30s）──► 主控 main

③ 信令（宿主仅转发）
主控 main ◄──WS /ws/remote-desktop?ticket──► 被控 host ◄──本机通道──► 被控 main（隐藏渲染器）
  monitors / offer / answer / ice / bye
  被控 renderer：getDisplayMedia（所选屏幕源）→ 视频轨
  主控 renderer：RTCPeerConnection 建立并消费画面

④ 媒体（P2P，不经宿主）
主控 renderer ◄══ WebRTC（DTLS-SRTP，UDP，host 候选直连，无 TURN）══► 被控隐藏渲染器

⑤ 输入（P2P 数据通道 → 注入）
主控 renderer：画面 pointer/keyboard → DataChannel
  control（有序可靠：按键/按钮/滚轮/文本）；pointer（unordered，可丢：移动采样）
被控 renderer → 被控 main → 宿主 services → Helper broker raw_input → SendInput

⑥ 指示与结束
被控 main：同意即起指示浮层；锁屏 → 冻结提示
结束路径：任一端断开 / 被控本地点「结束」或关开关 / 令牌或票据失效 / 应用退出
  → 关闭 PeerConnection 与隐藏窗口、撤销指示、清理票据，被控 host 记录会话结束（info 日志）
```

## 接口变更

1. **shared**：`serverRemoteInfoSchema.capabilities` 新增 `remoteDesktop?: z.boolean()`（optional，缺键=不支持，旧对端握手不报错，同 `computerUse` 先例）。新增 `packages/shared/src/remote-desktop.ts`：信令消息（monitors/offer/answer/ice/bye/error）、输入事件（pointer/key/text）、显示器矩形与坐标归一化纯函数（可单测）。
2. **lanAccessServer（host）**：新增 `POST /api/remote-desktop/session`（令牌鉴权 → 触发被控同意 → 签发票据；拒绝/超时/busy 返回稳定错误码）；新增 `GET /ws/remote-desktop?ticket=`（票据单次消费升级，双向转发信令）。票据实现复用 `hostCapability` 先例（30s TTL、单次消费、绑定 clientId）。
3. **被控 main**：新增会话协调器（同意弹窗、指示浮层、隐藏渲染器生命周期、`desktopCapturer.getSources({types:["screen"]})` 显示器枚举）；隐藏渲染器会话注册 `setDisplayMediaRequestHandler` 返回所选屏幕源（先例：`electronBrowserWebmRecorder` 的 handler 模式，但目标是屏幕源而非 WebFrameMain）；IPC 面 `rd:*`（listMonitors / startCapture / inject / sessionState / end），renderer 不触达 Helper。（本机 spike 已验证：隐藏窗 `show:false` + `backgroundThrottling:false` 可持续采集推流，27.5fps@1080p。）
4. **Helper（`packages/mode-cua` + `crates/mode-cua-ax`）**：broker 方法集新增 `raw_input`（鼠标绝对移动/按键/滚轮、键盘 down/up、Unicode 文本），复用 `input.rs` 既有 SendInput 原语，不走 CUA 动作语义（无 UIA 预检、无前台要求）；Helper 创建准入（`shouldCreateDefaultCuaProductHelper`）新增「远程桌面被控开关」为第二准入理由（与 CUA 插件启用并列，authority 与 identity 条件不变）；触发语义：被控注入在无 live helper 且被控开关开时，经与 CUA 相同的创建入口（同一生命周期）按需拉起，失败退避后仍报 `helper_unavailable`；打包链复用 `cua-helper`（win32-x64）不变。
5. **平台接口（renderer）**：设备列表复用既有 `getLanPairedPeers` / 发现结果；新增远程桌面会话控制（发起、结果回执、结束）经 main；令牌与 Helper 凭据不出 main。
6. **kill-switch**：新增打包层 env 开关（镜像 `MODE_CUA_PRODUCT_HELPER` 语义：只能关不能开）。被控开关默认关。

## UI 规则

- **侧边栏**：插件市场按钮下方新增「远程桌面」（`WorkspaceMainView` 新增 `"remote-desktop"`），仅 win32 desktop 显示（`isWindowsDesktop` 门）。
- **主视图**：
  - 设备页：已配对设备卡片（名称 / 平台 / 在线状态 / 能力位）；「连接」进入会话面；无配对设备 → 引导跳「设置 → 局域网访问」。
  - 会话面：视频（letterbox）、工具条（显示器切换、全屏、断开）、状态（延迟 / 帧率 / 分辨率）；断线终止并给出原因文案。
- **被控端**：同意弹窗（模态，发起设备名 + 「记住此设备」，60s 超时拒绝）；指示浮层沿用 `windowsCuaOperationIndicator` 的窗口形态先例；设置页「远程桌面」分区（总开关、已授权设备列表可撤销、监听端口/帮助说明）。
- i18n 中英同步；不改既有 key 语义。

## 安全边界

- 信任链 = 既有配对令牌 + 逐会话一次性票据；未配对/未同意不可达任何会话端点（401 / 稳定错误码）。
- 媒体强制加密（WebRTC DTLS-SRTP 自带）；信令在令牌通道内；不新增对公网监听（沿用既有端口）。
- 默认关闭 + 逐设备授权 + 会话中始终可见指示 + 本地随时终止；kill-switch env 可整链压死。
- 会话生命周期写 info 级日志（发起/同意/拒绝/结束），不记录画面内容。

## Rust/Tauri 迁移兼容性

桌面端将按 `docs/plans/2026-10-09-tauri-rewrite-roadmap.md` 迁移 Rust + Tauri v2（同仓绞杀者演进：React 前端保留进 webview、协议 schemars/typeshare 双端生成、LAN 域随 M3 用 `crates/lan/` 重写）。本功能的媒体方案（webview/Chromium 的 WebRTC）按该迁移做分层设计，不在当下自建 Rust 原生媒体栈：

- **直接迁移**：输入注入已是 Rust（`crates/mode-cua-ax` 的 rlib，迁移后直接链进 Tauri 核心）；协议契约（能力位/票据/信令消息/输入事件）为语言无关纯数据 schema，随 M1 双端生成路线演进。
- **零改动平移**：主控侧媒体与输入面全部是 Web 标准代码（RTCPeerConnection / DataChannel / video 渲染 / 输入采集与坐标映射），迁移后仍在 React webview 内运行；Windows 上 Tauri = WebView2（Chromium 内核），语义与 Electron 一致。
- **同形替换（唯一平台差异点）**：被控端采集源接入。Electron 为 `desktopCapturer + setDisplayMediaRequestHandler`；Tauri 侧为 WebView2 `ScreenCaptureStarting`（Tauri 公开 API 未暴露，需经 wry/`with_webview` 自研 COM 桥精确选源；未处理时的默认行为实现时验证）。
- **随宿主 Rust 化**：信令端点与票据转发是纯 HTTP/WS 语义，随 M3 `crates/lan/` 重写同形落地（验收口径对齐 `lan-remote-attach`）；会话编排（同意弹窗、指示浮层、隐藏采集窗）为宿主侧窗口与生命周期管理，随 Tauri 壳同形重建。

不锁死的线协议决策（现在就定，避免迁移时协议分叉）：
- **媒体契约 = 标准 WebRTC**（SDP/ICE/DTLS-SRTP/RTP + 标准编解码协商），不使用 Chromium 私有扩展；将来任何标准实现（如 Rust `webrtc-rs`）可互通或平替。
- **默认关闭 mDNS 候选混淆**（裸 host 候选）：既避开部分网络/防火墙下 `.local` 解析失败，也保证未来非 Chromium ICE 对端可直连（本机 spike 已验证候选为裸 IP；跨机可达性随 spike 两机清单验证）。
- **编码协商保持标准 SDP**：优先 H.264（Windows 硬件编解码普及）、VP8 兜底，避免未来 Rust 实现（openh264/MF）互通时语义变化（本机 spike 默认协商为 VP8；如需 H.264 硬件编码，实现时显式 `setCodecPreferences`）。

若本功能未赶上旧 Electron 线发布，可直接落在 Tauri/Rust 新栈（输入注入与协议已对齐），UI 面、协议面与验收场景不变。

## 降级与失败语义

- 旧对端无能力键 → 显示不支持；不建回退通道。
- 拒绝/超时/busy/令牌失效 → 稳定错误码与文案；不自动重试（由用户手动重连）。
- 锁屏：画面冻结 + 提示；解锁恢复（安全桌面不可采集）。
- Helper 不可用（kill-switch 关 / 未打包）→ 能力位为假，入口即挡，不做会话中兜底。
- 信令或媒体建立失败（含 ICE 超时）→ 会话终止并归因，指示与隐藏窗口同生命周期收口，不落半开状态。

## 已知限制（一期不变）

- UIPI：提权（管理员）窗口内的注入被系统拦截；不提供提权通道。
- Ctrl+Alt+Del 无法发送（SAS 限制）；Windows 登录界面不可操作。
- 本地用户与远程主控共享输入（无接管互斥）；无剪贴板、无音频。
- 键盘布局：主控与对端布局不一致时，字符输入以 Unicode 文本通道兜底，快捷键按 VK 语义。
- 实机注意：WebRTC UDP 可能触发 Windows 防火墙提示；不引入 TURN（局域网 host 候选直连，mDNS 决策见「Rust/Tauri 迁移兼容性」节）。

## 验收场景（待两台 Windows 实机）

- [ ] 配对 → 被控开关开 → 主控设备页出现设备且能力位为真；开关关/旧版对端显示不支持。
- [ ] 发起连接：被控弹窗；拒绝/超时主控收到稳定错误；允许后进入会话；会话中第二主控被 busy 拒绝。
- [ ] 「记住此设备」后再次连接不再弹窗；设置页撤销后恢复弹窗。
- [ ] 画面实时（目标 1080p30，局域网端到端延迟与画质 spike 测量后校准，目标 <100ms）。
- [ ] 鼠标移动/三键/滚轮/拖拽、键盘（字母/数字/快捷键/中文输入法文本）在对端生效。
- [ ] 注入目标 = 对端当前前台窗口（`raw_input` 无前台校验；对照 spike 实测：现 `perform` 路径因 `foreground_required` 不可直接复用）。
- [ ] 多显示器枚举与切换（会话内换轨不重建会话）；混合 DPI 坐标正确。
- [ ] 指示浮层在场；被控本地点「结束」立即终止；主控侧收到结束态；两端 UI 均无残留。
- [ ] 锁屏 → 冻结 + 提示，解锁恢复；提权窗口注入被拦截时有非阻塞提示。
- [ ] 票据重放/过期被拒；断开后隐藏窗口与 PeerConnection 无残留（进程与内存核查）。
- [ ] kill-switch 关：入口不出现、能力位为假；`pnpm typecheck` / `pnpm lint` / `pnpm architecture:check --changed` 全绿。
- [x] 单测：能力位 schema 缺键兼容、票据一次性与过期、输入事件编解码、坐标映射纯函数、会话状态机转移。（2026-10-12 实现轮已落地并全绿）

## 分期

- **P1（本 spec）**：Windows ↔ Windows 局域网实时画面 + 键鼠 + 同意/指示 + 多显示器切换 + 画质档。
- **P2**：剪贴板双向、输入互斥仲裁（与 CUA/本机用户）、自动重连、会话统计、画质档位（需先扩信令协议）、授权管理体验。
- **P3（另立 spec）**：macOS/Linux 对端（WebKit 系 webview 无 getDisplayMedia/WebRTC，届时需评估 Rust 原生媒体栈）、公网中继。
- **与 Rust/Tauri 路线图衔接**：P1 在现行 Electron 栈落地；若迁移先行，本功能作为 M3 LAN 域工作流的一部分直接落 `crates/lan/` 与 Tauri 新栈，协议与 UI 面不变。

## 实现进度（2026-10-12）

- 代码全量落地（T1–T6）：共享协议面、被控端信令端点与一次性票据、被控 main 会话协调器与宿主桥、`raw_input` 注入面、主控 UI（侧边栏菜单/设备页/会话面/设置分区/i18n）。
- 本机自动化矩阵：`pnpm typecheck` 0 错、`pnpm lint` 0 error、`architecture:check --changed` 0 违规；shared 54、ui 87、desktop 52、services 307（全部 0 fail）；helper 重建后打包一致性 8/8。
- 本机 spike 已验证：隐藏窗采集 27.5fps@1080p、ICE 候选为裸 IP、注入延迟 p50 2–4ms、中文文本逐字精确。
- **未执行**：两端 Windows 实机验收（步骤与判定标准见 [spike 记录](../plans/2026-10-12-remote-desktop-spike.md)）；画质档位、混合 DPI 校准、旧版 helper 的 `raw_input` 版本探测列为后续项。
