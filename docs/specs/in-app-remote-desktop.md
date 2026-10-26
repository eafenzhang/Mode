# 应用内远程桌面（自研媒体面 · 与系统 RDP 并存）

状态：已拍板（2026-10-15，用户决定「参考 RustDesk 重新自研」，并明确**不再保留系统 RDP 路径**）。
本文是远程桌面的**唯一权威**；v0.0.42 引入的系统 RDP（mstsc）路径随本决定一并移除
（见「取代系统 RDP」），其历史 spec 与实现见 `54aeb907`。

历史：本功能第一代（WebRTC）与第二代（DXGI + 硬件编码 + 自控传输）的设计与实现记录在
`54aeb907^` 的 `docs/specs/lan-remote-desktop.md` 与 `docs/specs/lan-remote-desktop-native-media.md`，
实现体同在该提交的父提交里。**本轮以恢复二代实现为基线**（我方自有代码，非第三方），
再按本文补齐缺口；不读任何第三方远控实现（见「与 RustDesk 的对标口径」）。

## 背景与决策

2026-10-15 上午把远程桌面改为「启动系统 RDP（mstsc）」（`54aeb907`，已发版 v0.0.42），
换来剪贴板/音频/多屏/SAS 与零媒体维护成本；代价是**三项只有自研路径才能提供的能力**：

1. **应用内画面**：RDP 画面在独立 mstsc 窗口，应用内没有会话面（画布/操作栏/状态条）；
2. **家庭版可被控**：RDP 主机需 Pro/Enterprise，家庭版彻底不可远程；
3. **应用级治理**：同意窗、逐设备授权、被控中指示条、被控中不可发起——Windows 凭据登录即同意，
   应用没有插入点。

用户据此决定：**恢复并继续自研媒体面**，与系统 RDP 并存，由用户在设备页选择。

## 取代系统 RDP（2026-10-15 追加决定）

**单一形态**：远程桌面只有应用内一条路径。v0.0.42 引入的系统 RDP（mstsc）路径——启动器、连接选项、
系统 RDP 状态探测、`systemRdp` 能力位、会话列表、`rd/connect|get-sessions|disconnect|*-rdp-options`
通道与其 UI/测试——**整体移除**，随本路径恢复的同一次变更交付。设备页对每台设备只有一个「进入桌面」入口。

相对 v0.0.42（已发布）用户可见的变化：

| 变化 | 方向 |
| --- | --- |
| 被控端系统要求 | Pro/Enterprise → **任意 Windows（含家庭版）** |
| 被控端本地用户 | 被远端登录顶下线 → **与远端共享输入**（本机用户留在会话里） |
| 画面位置 | 独立 mstsc 窗口 → **应用内画布**（可终止、可统计） |
| 同意与授权 | 无（Windows 凭据） → **应用级同意窗 + 逐设备授权 + 指示条** |
| 剪贴板 / 音频 / 多屏同时 / SAS | 系统自带 → **转入自研分期**（P1 逐项补齐；SAS 本轮明示不支持，见产品规则 11） |
| 维护成本 | 零（系统承担） → 自研媒体与注入链 |

对升级用户的影响需要说清：从 v0.0.42 升上来的用户会发现「系统 RDP」入口消失、原来记下的
`rdp:options:<serverId>` 偏好不再被读取（遗留凭据可不清，不影响运行）；需要剪贴板/音频/SAS 的用户
在本轮会暂时失去这些能力，直到 P1 落地——这是本次让渡的实质，写进发版说明。

## 范围与非目标

范围内：win32 + x64 两端；应用内实时画面（含系统光标）、键鼠控制、多显示器、剪贴板（文本）、
音频（被控→主控）、画质档位与自适应、同意/授权/指示、LAN 与 Tailscale 直连。

非目标（本轮不做，逐项对标见末节）：macOS/Linux 对端、手机 Web 端、公网 rendezvous/中继/打洞、
文件传输、虚拟显示器、打印机重定向、多会话 tab、WOL、Web 客户端、SAS 的特权服务化实现（见 P1）。
公网地址直连（公网优先 + LAN 回退）已单独落在
[lan-remote-public-endpoints.md](./lan-remote-public-endpoints.md)，不含中继与打洞。

## 产品规则

1. **被控端显式启用**：设置页「远程桌面（应用内）」分区，总开关默认关，win32 才显示；关闭时连接请求直接拒绝、不弹窗。
2. **唯一信任来源是既有 LAN 配对**：不新增第二套配对；未配对设备无法发起。首次连接由被控端弹「<设备名> 请求远程控制」（60s 超时=拒绝），可「记住此设备」写逐设备授权；授权管理与 LAN 配对记录同源（同一 client 记录的 `remoteDesktopAuthorized` 字段），撤销即刻生效。配对码与远程工作区、Mode Agent 共用同一个 `lanAccessServer.pairCode`（[remote-control-unified-pairing.md](./remote-control-unified-pairing.md)）。
3. **会话中指示常驻**：被控端顶层浮层「<设备名> 正在远程控制本机」+「结束」；闲置 4s 淡化、移入恢复；会话结束销毁窗口。锁屏/安全桌面无法采集时画面冻结并提示，解锁自动恢复。
4. **输入共享**：远端输入与本地键鼠共用同一物理会话，无接管互斥（与系统 RDP 语义相反，是家庭版可被控的前提）。
5. **被控中不可发起**：本机处于被控状态（口径 = 被控会话状态机 `phase !== "idle"`）时禁止再发起主控会话；权威闸门在主控 main，稳定错误码 `locally_controlled`。
6. **坐标映射与光标**：画面按视频实际尺寸 letterbox 适配；主控坐标以所选显示器源分辨率归一化传输，被控端换算为虚拟屏绝对坐标（复用 `input.rs` 的 VIRTUALDESK 归一化）；混合 DPI 多屏按源分辨率独立处理；画面内显示对端光标（采集自带合成），主控隐藏本地光标避免双光标。
7. **剪贴板（文本）**：会话面操作栏提供开关，默认开；**只同步文本**（CF_UNICODETEXT 口径，与既有 paste 同一实现面），文件与图片不在范围内；单条上限 128K 码元，超限两个方向都忽略（不下发、也不落地）。**双向均已上线**（被控 → 主控与被控 ← 主控）：任一端剪贴板变化经媒体 WS 的 `{t:"clip"}` 送到另一端并写入。回环抑制的判据是唯一一条：**已经同步过的值再被观测到不算变化**——无论它是本端刚写进去的还是刚收到并应用的。主控侧读本机剪贴板走 `rd/read-clipboard-text`（Tauri 壳的 Rust 剪贴板命令），不用 renderer 的 `navigator.clipboard`（读剪贴板受 clipboard-read 权限约束）。
8. **音频**：被控端系统声音单向送到主控（对标 RDP `audiomode=local`）；会话面提供开关，默认开；编解码为 Opus；不提供麦克风上行。
9. **多显示器**：默认「跟随被控端当前主屏」，可选「全部铺开」（多路流或单张虚拟屏合成，实现层决定）；选择入口在会话面操作栏。
10. **画质档位**：清晰/均衡/流畅三档（fps、maxWidth、码率见实现层参数），会话面可切；会话中改档 = 重建会话（既有链路），拥塞时自动降档只降不升。
11. **SAS（Ctrl+Alt+Del）不提供，且界面明示**：Windows 禁止用户态进程合成 SAS，需要被控端安装服务并放行
    `SendSAS` 策略（对标 RustDesk 的 service 组件；我方没有任何 Windows 服务设施）。会话面操作栏给一个
    **禁用按钮 + 原因 tooltip**——既不是点了没反应的假按钮，也不是静默缺失；服务化列为独立立项候选。
    **同理不可达的还有被控端 UAC 提权窗口上的输入**（`input.rs` 已有 `uipi_preflight` 承认这一点），
    属已知限制而非缺陷。
12. **降级与失败可见**：编码器**硬编优先、软件兜底**——硬件口径枚举不到可激活的编码器 MFT 时，回退系统自带软件编码器继续出画面（设置页提示「已回退软件编码」，**不算失败**）；**硬软两路都不可用**才报错。采集 ACCESS_LOST、锁屏、网络断开各自有稳定错误码与状态条文案，不用超时掩盖。
13. **握手有界（2026-10-26 追加）**：WS 建连后到**对端第一帧**的上限 15s（`handshakeTimeoutMs`，
    可注入）。超时按 `unsupported` 收口且**不可重试**，Host 日志写明「对端未回任何信令帧」。
    这不是用超时掩盖同步问题：旧对端的信令 schema 把新版本 `hello` 当非法帧**静默丢弃**
    （不回报错、不关连接），主控没有任何可观测事实可依，只能永久挂着——有界失败至少把
    「连不上」变成可行动的错误（UI 文案已指引「对端未开启或版本过旧，升级后重启」）。
    实测（2026-10-26）：对端 `0.0.0-dev` 旧构建 `protocolVersion: 1`，主控 v2 hello 被丢弃，
    会话在对端一直占用（后续请求 `busy`）。**对端必须与主控同版本**。

## UI 规则（2026-10-20 统一设备列表）

远程控制页只有一个设备列表：LAN 已配对设备（同时代表远程工作区与远程桌面能力）、
Mode Agent 设备、发现的未配对 LAN 设备同屏。设备行采用紧凑行（名称 / 地址 / 状态胶囊 /
类型徽标 / 动作），不再使用 16:9 预览卡；页面顶部只保留「生成配对码 / 添加设备 / 刷新」
一组动作，配对码也只有桌面 LAN 一次性码这一套。结构与动作的权威定义见
[remote-control-unified-pairing.md](./remote-control-unified-pairing.md)；本文的连接、同意、
授权与媒体规则不变。

## 状态所有者

| 状态 | 唯一所有者 | 说明 |
| --- | --- | --- |
| 被控总开关 / 逐设备授权 | 被控端设置（settings service） | 设置页写；同意窗「记住」写同一字段 |
| 配对与设备令牌 | 既有 lanAccess 凭据服务（不变） | 既有配对流程 |
| 能力位 `remoteDesktop` / `systemRdp` | 被控 host（`createServerInfo`） | 握手即时计算、无持久化；缺键=不支持 |
| 会话票据（信令 + 媒体） | 被控 host | 一次性签发、30s、单次消费、绑定 clientId |
| 被控会话状态机 | 被控 main（coordinator） | idle → 请求中 → 等待同意 → 进行中 → 结束（单向，失败即收口） |
| 主控会话状态机 | 主控 main（信令与生命周期）+ renderer（媒体/输入/UI） | renderer 面驱动，main 转发信令 |
| 媒体密钥 + 会话盐 | 被控 main（同意通过时生成） | 只在被控 main/helper 与主控 renderer 内存；不落盘不进日志 |
| 采集/编码会话 | 被控 helper（`rd_media` 模块） | start/stop 收口；ACCESS_LOST → 状态事件 |
| 输入注入 | 既有 CUA Helper（唯一 SendInput 实现） | broker `raw_input`；主控经 host 转发 |
| 剪贴板文本门（回环抑制 + 开关） | 被控 host 媒体中继（被控侧）+ 主控 renderer（主控侧） | 每端一份门（shared 同一状态机）；helper 只上报「变化才发」，不持判据；开关经媒体 WS 上行 `clip-mode` |
| 音频采集/编码 | 被控 helper（进程内 WASAPI 采集 + Opus 编码） | 与视频同一条传输；主控 renderer 解码播放 |
| 显示器枚举与选择 | 被控 main（coordinator） | 每条 SelectMonitor 现算下发，不缓存 |

## 事件顺序

```text
① 设备与能力（复用既有发现/配对）
被控 host  createServerInfo.capabilities = { remoteDesktop, systemRdp{enabled,editionOk} }
主控设备页 getLanPairedPeers() + discoverLanPeers()；能力位按需经 server-info 拉取

② 连接请求与同意
主控 main ──POST /api/remote-desktop/session（设备令牌 + quality + codec）──► 被控 host
被控 host ──信令帧──► 被控 main coordinator ──同意窗──► 用户
同意 → 生成{key,salt} + 双票据 ──► host；拒绝/超时 → error 收口

③ 媒体建立
主控 main ──rdStartSession 返回{mediaUrl,ticket}──► renderer
renderer ──信令 WS（hello v2 → monitors → select-monitor）──► 被控 host ──► coordinator
coordinator ──media-open{key,salt}（首个 select-monitor 应答）──► renderer
renderer ──连媒体 WS（mediaTicket）+ {t:"ready"}──► 被控 host ──► helper 开流
helper 采集(DXGI) → 硬编(MF) → 逐帧 AES-256-GCM → 本机 TCP → host 中继（不解密）→ renderer
renderer 解密 → VideoDecoder → 最新帧直画 canvas（丢旧帧，自建零缓冲）

④ 输入与反馈
renderer InputCapture ──媒体 WS 上行 JSON──► host ──broker raw_input──► helper SendInput
renderer ──{t:"ping"}──► host 回 {t:"pong"}（RTT 不走 helper）
helper ──采集状态事件──► host ──信令 freeze/unfreeze──► renderer

⑤ 剪贴板与音频（P1）
剪贴板：helper 轮询文本变化 → 信令板外通道 → renderer 写剪贴板（带来源标记防回环）
音频：helper WASAPI 采集 → Opus → 与视频同一管道（独立 kind）→ renderer WebCodecs/AudioDecoder 播放

⑥ 收口
主控「断开」/ 被控「结束」/ 任一端异常 → bye → 双方停采集、销票据、销毁指示条；会话记录移除
```

## 接口（相对 v0.0.42 的增量）

- **能力位**：`server-info.capabilities.remoteDesktop` 语义为「应用内远控可用」（win32 && 被控开关 && kill-switch，
  握手即时计算、无持久化、缺键=不支持）；**`systemRdp{enabled,editionOk}` 及其注册表探测一并删除**
  （`host/systemRdpState.ts`、`server-remote.ts` 的类型、server 侧注入）。
- **IPC（`rd/*`）**：回到自研面 `rd/get-peer-capability`、`rd/start-session`、`rd/get-controlled-state`、
  `rd/send-signal`、`rd/end-session`、`rd/session-event`、`rd/get-server-state`、`rd/authorize-client`、
  `rd/revoke-client`，以及 P1.1b 新增的 `rd/read-clipboard-text`（读本机剪贴板文本，供剪贴板上行观测）；
  **删除 RDP 面** `rd/connect`、`rd/get-sessions`、`rd/disconnect`、`rd/get-rdp-options`、
  `rd/set-rdp-options`。四处同步纪律不变：main 常量 → preload → shared `IPlatformService` → renderer 桥。
- **协议**：信令 union v2 恢复（hello/monitors/select-monitor/bye/error/media-open/freeze/unfreeze/
  request-media-ticket）；媒体 WS 帧格式与 IV 构造沿用二代设计（见实现层）。
- **消息契约**：`main ↔ host` 的 RD 消息族回到二代集合（被控设置面 + 主控对端探测 + 媒体/信号族），
  **v0.0.42 的 `systemRdp` 消息字段删除**；两份镜像文件仍由
  `packages/desktop/tests/remote-desktop-host-messages.test.mts` 守卫（须同步扩展断言）。

## 安全边界（不可妥协）

- 媒体密钥与会话盐只在被控 main/helper 与主控 renderer 内存；host 只转发密文，**明文帧与密钥都不经过 host**。
- 令牌与输入注入不出 main/host；renderer 不得经通用 RPC 触达 Helper。
- 逐帧 AES-256-GCM，IV = 会话盐(8B) ∥ u32 大端 seq；每会话唯一 key，IV 不重复。
- 日志不写密钥、盐、帧内容与用户数据。

## 实现层（可演进，基线 = 恢复二代）

采集 DXGI Desktop Duplication（含 user32 光标兜底与指针轮询补帧）、编码 MF 硬件 H.264/HEVC
（GOP 2s + 换屏强制 IDR、码率随档位）、传输 长度前缀二进制 + AES-GCM + 本机 TCP + LAN WebSocket 中继、
解码 renderer `VideoDecoder` + canvas、输入 `raw_input`。**以上细节以恢复的实现与其测试为准**，
本文不重复参数（避免两处真相）；参数变更须同步 `shared/remote-desktop.ts` 的档位预设与本节。

### 音频（P1.2，设计已定、实现未开始）

- **采集**：被控 helper 进程内 WASAPI **回环采集**（loopback，抓系统播放声音；不做麦克风上行）。
- **编码：先不做 Opus，走「16-bit PCM 直传」**。理由：① 我方范围是 LAN/Tailscale，48kHz 立体声
  16-bit ≈ 1.5 Mbps，相对 8 Mbps 视频预算是可接受的增量，而 LAN 拥塞时先降视频档；② 纯 Rust 实现，
  **不引入需要 C 工具链构建的依赖**（当前 helper 的 Rust 侧是纯 Rust，加 libopus/Opus 绑定会把构建
  风险从"编译错误"扩大到"交叉工具链"，而 Rust 侧我只能靠 CI 判定）；③ renderer 侧直接喂 WebAudio，
  **连解码步骤都省掉**。Opus 作为 P2 升级项（弱网/公网音频时再议），届时按第三方台账引入。
- **传输**：与视频同一条媒体 WS，但**独立通道标识**（二进制帧头加 `channel` 字节：0=视频 1=音频），
  各自独立 seq 与 AES-GCM IV（音频不与视频共用 seq，否则一方的丢帧会污染另一方）；不混进视频管道——
  音频丢帧策略是**丢弃不请求关键帧**，视频才有 IDR 重同步。
- **播放**：renderer 用 WebAudio（`AudioContext` + 环形缓冲）播放，按到达节奏平滑；缓冲上限固定
  （超过即丢最旧，宁可断音也不让延迟累积）。
- **失败语义**：采集不可用（无声卡/驱动拒绝）→ 会话内音频降级为静音并在状态条提示；**绝不影响画面与输入**。
- **开关**：会话面操作栏（默认开），与剪贴板开关同形（`clip-mode` 的先例）。

### 多显示器「全部铺开」（P1.3，设计已定、实现未开始）

DXGI Desktop Duplication 是**按输出（output）** 的，一张 DUPL 只能抓一个显示器；因此"全部铺开"要么
① 每个输出各开一路采集+编码（多路流，传输与解码都要扩成多路），要么 ② 在被控侧合成一张虚拟桌面
（跨输出的合成需要布局信息与额外显存拷贝，且超过单编码器输入尺寸上限时要缩放）。
**选 ② 的简化版**：仅当所有显示器拼成的虚拟桌面尺寸 ≤ 编码器上限（1920×1080 档按各屏等比缩放后再拼）
时提供"全部铺开"，否则置灰并提示。多路流（①）列为 P2——它会同时把传输、解码、统计三处扩成多路，
一次改动面太大。

### SAS（P1.4，已完成）

会话面操作栏的禁用按钮 + 原因 tooltip（产品规则 11）。不含任何后端能力——正因为做不到才要明示。

## 验收场景（两台 Windows 实机，用户执行）

- [ ] 家庭版被控端：可连接并出画面——本路径存在的首要理由（系统 RDP 做不到这点，已移除）。
- [ ] Pro 被控端：同样可连接；应用内出现画布与操作栏（无独立 mstsc 窗口）。
- [ ] 首次连接弹同意窗；「记住此设备」后不再弹；设置页撤销后重新弹。
- [ ] 被控端指示条常驻，点「结束」立即收口；闲置淡化、移入恢复。
- [ ] 延迟与跟手：LAN 下 `fps` 与 `帧龄` 稳定，鼠标移动画面跟手（对标二代验收口径）。
- [ ] 多显示器：切换显示器生效；「全部铺开」两块屏同时可见。
- [ ] 剪贴板：本机复制 → 远端粘贴；反向亦然；连续复制不振荡。
- [ ] 音频：被控端播放声音能在主控端听到；关闭开关后静音。
- [ ] 画质档位：会话面切档后以新档恢复（不再需断开重连）。
- [ ] SAS：会话面明示不支持（不得静默失败）。
- [ ] 无硬件编码器机器：回退系统软件 H.264 编码器并正常出画，设置页提示「已回退软件编码」；硬软两路都不可用时状态条提示根因，不黑屏假连。
- [ ] `pnpm typecheck` / `pnpm lint` / `pnpm architecture:check --changed` 全绿；`cargo test --manifest-path crates/mode-cua-ax/Cargo.toml` 全绿。

## 分期

- **P0 恢复基线**：把二代实现与测试按本 spec 恢复，并在**同一次变更**里移除 v0.0.42 的系统 RDP 路径
  （`revert 54aeb907` 即同时完成两件事：恢复 68 个删除文件 + 删除全部 RDP 代码）。**验收 = 与 v0.0.35 时代能力对齐**
  （除本 spec 明确改动的部分），且 `systemRdp` 能力位、RDP IPC 通道、RDP UI 与测试全部消失。
- **P1 补齐缺口**：剪贴板文本双向、音频（被控→主控）、多显示器全部铺开、SAS 现状明示；
  会话面开关与 i18n 三份。
- **P2 质量**：传输层抽象出可替换点（为 UDP/抗丢包预留）、动态分辨率、弱网自适应阈值实测标定。
- **P3 组网**：公网端点（公网优先 + LAN 回退）已落在
  [lan-remote-public-endpoints.md](./lan-remote-public-endpoints.md)；公网 rendezvous + 中继 + 打洞
  仍未实现（若需要，另立 spec）。
- **P4 周边**：文件传输、虚拟显示器、多会话 tab、打印机重定向——逐项单独评估，非承诺。
- **不做**：macOS/Linux 对端、Web 客户端、WOL、跨平台虚拟屏、自研 RDP 协议。

## 与 RustDesk 的对标口径

**只参考其公开资料（README、Cargo 清单、官方文档）与黑盒行为，不读取其实现源码。**
理由是许可与纪律双重：RustDesk 为 **AGPL-3.0**（引入其代码会把整个产品置于 AGPL），
且仓库既有结论是「不引入 RustDesk 等第三方远控实现」（见 `clean-room-policy.md` 精神与 `54aeb907^` 的
`lan-remote-desktop.md` 非目标节）。对标只用于**确定目标能力**，不用于取实现。

| RustDesk 公开能力 | 我们 | 归属 |
| --- | --- | --- |
| 应用内画面（自绘） | 二代已有 | P0 恢复 |
| 多显示器（铺开/切换） | 切换已有；铺开缺 | P1 |
| 剪贴板（文本/文件） | 缺（文本 P1；文件不做） | P1 / 不做 |
| 音频（被控→主控，Opus） | 缺 | P1 |
| Ctrl+Alt+Del | 缺，且用户态不可达 | P1 明示不支持；服务化另立项 |
| 画质档位 / 自适应码率 | 二代已有（动态降档） | P0 恢复 |
| 硬件编码矩阵（H.264/H.265/AV1） | H.264 + HEVC（MF） | P0 恢复；AV1 不做 |
| 自建 rendezvous + 中继 + 打洞 | 缺（只有 LAN/Tailscale 直连） | P3 |
| 文件传输 | 缺 | P4 |
| 虚拟显示器 | 缺 | P4 |
| 多会话 tab | 缺（一会话一窗口） | P4 |
| 跨平台（mac/Linux/Android/iOS/Web） | 不做 | 不做 |
| 打印机重定向 / WOL / 摄像头 | 不做 | 不做 |

## 实现进度

- [ ] P0 未开始（本文与 [计划](../plans/2026-10-15-in-app-remote-desktop.md) 为起点）。
