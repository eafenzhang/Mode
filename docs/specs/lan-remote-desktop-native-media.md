# LAN 远程桌面 · 原生媒体面（DXGI + 硬件编码 + 自控传输）

> 接管 `lan-remote-desktop.md` 的**媒体面**（采集/编码/传输/解码/输入通道）；产品规则（同意窗、指示条、能力位、设置面、坐标映射、光标、安全边界总则）仍以该 spec 为准，本文只替换其实现层与对应事件顺序段。
>
> 决策记录（2026-10-12，用户拍板）：**纯原生直替，不留 WebRTC 回退**；拓扑 = 方案 A（helper 采集硬编 → host 中继 → renderer 直连加密 WS → WebCodecs 解码）；UDP 传输与画质档位仍为后续迭代。

## 背景与动机

WebRTC 屏幕共享的端到端延迟由浏览器栈决定：30/60fps 帧量化 + 编码调度 + **接收端抖动缓冲** + 自适应码率节流，实测跟手感上限约 80–150ms，达不到主 spec 验收目标「端到端 <100ms」。输入侧（rAF 节流、批处理）已在一期削减完毕，剩余延迟全部在媒体管线。

原生方案直接替换三层：DXGI Desktop Duplication 采集（系统同源、无浏览器拷贝）、Media Foundation 硬件 H.264 编码（Win10 自带、低延迟 preset）、自控 LAN 传输（无抖动缓冲，解码即画）。延迟预算 ≈ 采集 1 帧 + 硬编 ~5ms + LAN <2ms + 硬解 1 帧 ≈ **25–40ms**。

## 范围

**替换**：采集、编码、传输、解码渲染、输入通道、信令中的媒体协商消息。

**保留（零改动或仅接线）**：LAN 配对与逐设备令牌、同意窗与指示条、`server-info` 能力位、显示器枚举与切换、`raw_input` 注入、RD-only Helper 按需拉起与 kill-switch、稳定错误码与日志纪律、会话面 UI（操作栏/状态条/全屏）。

**不做**：UDP 传输与抗丢包（P2，传输层预留替换点）、画质档位切换（仍 P2，需扩信令）、非 Windows、WAN 优化、自动重连。

## 总体架构

```text
被控端
  crates/mode-cua-ax/src/rd_media/（新，napi 导出）
    DXGI Desktop Duplication 60Hz ── 光标形状/位置合成进帧（GPU，产品规则 7）
         └► D3D11 Video Processor 出 NV12 ─► Media Foundation「H.264 HW」编码
              （GOP 2s + 换屏强制 IDR；码率 8Mbps；降级预案见「编码」）
  rdMediaStart/帧回调(THREADSAFE FN)/Switch/Stop ── napi
  helper Node（entry/server.mjs）：每帧 AES-256-GCM 加密 → 长度前缀二进制
         └► 本机 TCP（被控 host 每会话监听的中继端口）
  被控 host：中继（透传密文，不解密）── LAN WebSocket
         /ws/remote-desktop-media?ticket= （复用既有票据鉴权升级先例）
  ▲ 输入反向上行：同一条 media WS（JSON 输入事件）→ host → 既有 broker raw_input → helper

主控端
  renderer：media-open{key,salt}（经既有信令链下发；连接地址 mediaUrl 由 rdStartSession 直达）→ 连 media WS
    下行解密(WebCrypto AES-GCM) → VideoDecoder(H.264 硬解) → 最新帧直画 canvas
    （丢旧帧自绘 = 自建零缓冲，替代 WebRTC 抖动缓冲）
    输入：InputCapture 采集不变，sink 改为 media WS 上行

信令（不变的骨架）
  同意窗/指示条/hello/monitors/select-monitor/bye/error 走既有信令 WS
  （主控 renderer ↔ 主控 main ↔ 被控 host ↔ 被控 main coordinator）
```

## 状态所有者

| 状态 | 唯一所有者 | 说明 |
| --- | --- | --- |
| 会话媒体密钥（32B）+ 会话盐（8B） | 被控 main（coordinator） | 同意通过时 `crypto.randomBytes` 生成；经信令下发；仅存在于被控 main/helper 与主控 renderer 内存，不落盘、不进日志 |
| 采集/编码会话（DUPL + MF） | 被控 helper（`rd_media` 模块） | `rd_media_start` 开、`rd_media_stop` 收口；ACCESS_LOST → 状态事件 |
| 帧加密 | 被控 helper Node | 逐帧 AES-256-GCM，IV = 会话盐(8B) ∥ u32 大端 seq，seq 单调递增（每会话唯一 key，IV 不重复） |
| 中继端口与 media WS | 被控 host | 每会话监听 127.0.0.1 随机口；media WS 走 LAN server 升级（票据单次消费、30s、绑定 clientId）；中继不解密 |
| 解码/绘制/帧率统计 | 主控 renderer | VideoDecoder + canvas；fps = 实际绘制帧率 |
| RTT 统计 | 被控 host（pong 应答） | 上行 `{t:"ping",ts}` → host 回 `{t:"pong",ts}`，不经 helper |
| 信令状态机 | 主控 main `controllerSession` + 被控 `coordinator` | 不变（仅 union 增删） |
| 输入事件源 | 主控 renderer `InputCapture` | 采集逻辑不变，sink 换 media WS |
| 输入换算基准（monitor 物理 rect + 虚拟屏 union rect） | 被控 main（coordinator 每条 MediaStart/Switch 现算） | 随契约 F 下发、被控 host 持有，用于 `{t:...}` 归一化事件 → VIRTUALDESK 绝对坐标换算；不缓存（显示器变化由下一条消息自动带新值） |

## 协议

### 信令 union（protocolVersion 2）

- **保留**：`hello{protocolVersion:2}`、`monitors`、`select-monitor`、`bye`、`error`。
- **删除**：`offer`、`answer`、`ice`（WebRTC 消费者随媒体面移除）。
- **新增**：
  - `media-open{key, salt}`（被控 → 主控，随**首个 `select-monitor` 的应答**下发——此时
  主控信令 WS 已 attach，信号不会被「无会话连接」丢弃，见事件顺序 ③）：`key`/`salt` 十六进制；
  - `mediaUrl`（连接地址 + mediaTicket）不走信令：由主控 host 从 POST 响应拼出，
  经 `rdStartSession` 返回值直达 renderer（被控 host 不知道自身对外可达地址）。
  - `freeze{reason:"locked"|"access-lost"}` / `unfreeze`（被控 → 主控，DUPL 丢失/恢复时），复用既有「锁屏画面冻结 + 提示，解锁恢复」语义与文案。
  - `request-media-ticket`（主控 → 被控）/ `media-ticket{mediaTicket}`（被控 → 主控）：媒体 WS 瞬断重连（P2-1）的重签对上；被控在会话 WS 上本地处理（不落信令转发面），主控胶水在 dispatch 前拦截回帖。
- **版本门（纯直替的唯一兼容闸）**：
  1. `server-info.capabilities` 新增 `remoteDesktopNative?: true`——旧对端缺键 → 设备页既有「不支持（版本过旧）」文案，不发起会话；
  2. `hello.protocolVersion !== 2` → 回 `error{code:"unsupported"}` 收口。

### media WS 消息

- 升级：`GET /ws/remote-desktop-media?mediaTicket=`，复用既有票据 store（新增第二张票，与信令票各自单次消费、同 clientId 绑定；POST `/api/remote-desktop/session` 响应改为 `{ticket, mediaTicket}`）。
- **下行二进制帧**（host 透传 helper 密文）：`[u32 seq 大端][u8 flags(bit0=关键帧)][ciphertext ∥ GCM tag]`；**IV 不随帧传**，解密方用 `salt(8B) ∥ seq(4B 大端)` 重建（与状态所有者表、安全节同一口径）。
- **下行 JSON**：`{t:"freeze"|"unfreeze"|"pong"}`。
- **上行 JSON**：既有输入事件 schema（`RemoteDesktopPointerEvent` / `RemoteDesktopControlEvent` 原样复用）+ `{t:"ping",ts}`、首条 `{t:"ready"}`（decoder 配置完成的开流确认；host 收到后经本地管道向 helper 下发开流指令，helper 收到前只采集不推流——避免中继在无 WS 时缓冲堆积）。
- 加密边界：**明文密钥与明文帧都不经过 host**——helper 加密后出站，host 只转发密文，renderer 用 WebCrypto 解密。

### 本地管道（helper ↔ 被控 host）

长度前缀二进制：`[u32 len][u8 kind][消息]`，同机不加密；`kind=0` 帧密文（helper→host）、`kind=1` 采集状态事件（`frozen/unfrozen`，helper→host，host 转成信令 freeze/unfreeze）、`kind=2` 开流指令（host→helper，ready 后下发）。带宽小的控制仍走既有 JSON-RPC broker（`rd_media_start/switch/stop` 参数与状态回调）。

### 画质档位与编码能力缓存（P1-3，2026-10-13）

- **画质档位**：主控在**会话面顶部操作栏**选择「清晰 / 均衡 / 流畅」（`shared/remote-desktop.ts` 的
  `REMOTE_DESKTOP_QUALITY_PRESETS`：fps 60/60/30、maxWidth 1920/1920/1600、码率 8/5/3Mbps、
  自适应低档 4/2.5/1.5Mbps）；选择记忆在设备页级（本会话运行内跨连接保持），连接时以该档位
  随 `POST /api/remote-desktop/session?quality=<档位>` 下发（缺省/
  非法值回默认档 `sharp` = P1-3 之前的硬编码参数，旧主控不带该参数行为不变）。被控在
  `rd_media_start` 按预设出参数，替代旧硬编码 1920/60/8Mbps；主控侧自适应码率的两档
  改为「预设码率 ↔ 预设低档」（拥塞不越级到其它档）。运行期只切码率（`set-bitrate`），
  fps/maxWidth 在会话建立时定型——**会话中改档 = 立即重建会话**（2026-10-13 选择入口由
  设备卡移至会话面操作栏）：本地 bye 结束旧会话 → 走 P2-1 重连编排重 POST，被控 90s
  窗口同设备免二次同意，busy/瞬时错误按既有退避自动重试；选择器在未连接/重连进行中禁用。
  **动态降档（2026-10-13）**：低档码率仍**持续**拥塞（连 2 个 3s 采样窗解码丢帧 ≥3
  且 RTT ≥120ms——先让码率两档吸收，不动辄重建）时自动以更低画质档重建会话
  （复用改档重建链路，状态条经历「正在恢复」；30s 冷却，仍拥塞可再降一档）。
  **只降不升**：升档是用户在操作栏的手动动作（自动回升会在阈值附近振荡）。
- **编码能力缓存**：被控 host 从**真实会话**的 `rd_media_start` 结果观测硬件编码器可用性
  （成功 = available；错误含「无可用硬件 H.264 编码器」= unavailable；无会话发生 = unknown），
  **持久化（2026-10-13，对标 UU capability cache）**：观测值落 app 配置目录
  `rd-encoder-capability.json`、host 启动回读——冷启动即可显示能力提示（无需等首次会话）；
  不按 GPU 指纹分卡、不预探测：GPU/驱动变更后的过期值由**下一次真实会话观测自愈覆盖**；
  读取白名单收敛、损坏文件按无缓存。
  随 `RemoteDesktopServerStateResult.encoderCapability` 暴露给设置页——unavailable 时显示
  「未检测到硬件 H.264 编码器」提示（连上但不出画面的根因自诊断）。缓存为 host 进程级
  内存（每次启动从零观测），不额外拉起 helper 做预探测（探测动作与首次会话共用同一路径）。

## 被控 Rust（`crates/mode-cua-ax/src/rd_media/`）

- **采集**：DXGI Desktop Duplication（`windows` crate，与 `screen.rs` 同依赖面）；输出分辨率 = 显示器原生（>1080p 时由编码链缩放，见下）。
- **光标**：DUPL `PointerPosition` + `PointerShape` 合成进帧。**GPU 合成已落地（P0-2b，2026-10-13）**：形状变更时渲染成小 BGRA 覆盖层纹理（透明像素 alpha=0），每帧经双流 `VideoProcessorBlt`（流 0 = 主画面——桌面更新帧直接以采集帧纹理为源（P0-3，不再 frame→clean→proc 两次整帧拷贝）；流 1 = 光标覆盖层，`VideoProcessorSetStreamDestRect/SourceRect` 定位、per-pixel alpha 混合）合成到 proc，全程零回读。初始化或运行中任一步在驱动上失败 → **整会话回退 CPU 回读路径**（P0-2a：回读背景 + CPU 混合 + 回写），对外协议与行为不变。驱动行为由 `capture.rs` 内 `gpu_cursor_tests` 在真实硬件设备上验证（per-pixel alpha/覆盖矩形/背景保留；CI 无硬件设备跳过）。
  - **形状类型语义（以 MSDN `DXGI_OUTDUPL_POINTER_SHAPE_TYPE` 为准，2026-10-13 修正）**：
    1. `MONOCHROME`（1）：1bpp AND 掩码 + 1bpp XOR 掩码（高半/低半）——既有实现不变；
    2. `COLOR`（2）：**32bpp ARGB，alpha 通道即形状**——`visible = alpha>0`、栅格保留真实 alpha
       （此前强推 alpha=255 + 全可见会把带透明通道的光标画成不透明色块）；无 alpha 信息的位图源
       不走此路径，由 user32 兜底的 AND 掩码处理；
    3. `MASKED_COLOR`（4）：**32bpp ARGB、alpha 位即掩码，无尾随 1bpp 掩码**（0=替换、0xFF=XOR）——
       此前按「颜色位图+尾随 AND」解析，长度校验必失败 → 该形状整体跳过 = 光标不可见。
  - **XOR 语义**：`MASKED_COLOR` 的 0xFF 掩码与 0x00 掩码必须分别 XOR/替换到**背景**上——
    GPU 双流覆盖层只表达 alpha 混合，无法 XOR；**含 XOR 像素的形状整形状走 CPU 补丁路径**
    （背景回读本就是 CPU 路径的输入；该类形状少见，代价可控）。CPU 补丁混合按形状逐像素
    走：XOR（bg ^ color）/ 真 alpha 混合 / 替换 / 保留背景。
  - **user32 光标兜底（2026-10-13 新增）**：软件光标/虚拟显示环境（IDD 虚拟屏、远控软件接管
    显示等）下 **DUPL 完全不投递指针数据**（实机探针：200+ 次指针移动 `LastMouseUpdateTime=0`、
    `PointerShapeBufferSize=0`），`shape=None` → 光标永不合成（实机现象「画面里没有光标」）。
    规则：`dupl_shape_seen` 标记——DUPL 从未投递过形状时，每帧解析指针状态改经 user32
    （`GetCursorInfo` 定位/可见性 + `GetIconInfo`/`GetDIBits` 取位图：32bpp 有 alpha 用真 alpha、
    无 alpha 用 AND 掩码、单色走既有 render 布局；形状按 HCURSOR 句柄缓存，仅位置每帧采），
    喂入与 DUPL 完全相同的 `CursorShapeRaster` 合成管线（GPU/CPU 路径与几何换算零分叉）；
    DUPL 一旦投递形状即回归 DUPL 权威（覆盖 user32 缓存，不再采样）。
    零投递环境同时缺失**指针事件**（光标移动不产生任何 DUPL 帧）：`AcquireNextFrame` 超时
    改由 user32 快照**轮询补帧**——快照判重 / 15ms 节流 / clean 重合成与 DUPL 指针-only
    分支同语义；否则远控侧动鼠标画面纹丝不动、只有桌面像素恰好变化才刷新（2026-10-13
    实测「连上但卡死」根因；harness 用动画窗口产帧会掩盖此洞）。正常环境不走轮询。
- **编码**：MF 视频编码器（`MFT_ENUM_FLAG_HARDWARE` 选「H.264 HW」，Win10 系统自带）；输入 D3D11 纹理 → D3D11 Video Processor 出 NV12（承担 >1080p 缩放）→ MF `MFT_MESSAGE_SET_D3D_MANAGER` 全 GPU 路径。参数：平均码率 8Mbps、GOP 120 帧（≈2s@60fps）、`select-monitor` / 分辨率变化强制 IDR。
  - **HEVC 选型（2026-10-13，对标 UU 按 GPU 协商 H.265）**：主控发会话请求前用
    `VideoDecoder.isConfigSupported` 预检本机 HEVC 解码，`rdStartSession` 携 codec 偏好
    （`h264`|`hevc`，缺省/旧主控 = `h264`）→ `POST /api/remote-desktop/session?codec=` →
    被控 host 记入会话（`sessionCodec`）→ `rd_media_start` 参数 `codec` 直达 helper/napi。
    helper 侧 `codec=hevc` 时按输出 subtype=HEVC 枚举硬件 MFT；**无硬件 HEVC 编码器
    回退 H.264**（warn，不失败）——偏好与实际不一致是安全的，因为**解码配置串始终由
    关键帧码流自描述派生**（主控 `codecStringFromAnnexB` 扩展：HEVC VPS/SPS 解析 →
    `hev1.<idc>.<compat hex>.<tier><level>.<constraint hex>`，同款「新参数集触发重配」）；
    keyframe 权威 `contains_idr` 分 codec（HEVC 按 NAL type 16..21 = BLA/IDR/CRA）。
    两端独立降级兼容：旧被控忽略 `codec` 查询参数走 H.264、旧主控不带参数默认 H.264、
    主控不支持 HEVC 解码时偏好即 H.264——三向都不需要版本门。
  - **异步 MFT（2026-10-13 修复）**：硬件 H.264 MFT（NVENC / AMF / QSV）普遍是**异步编码器**，非 MF 管线直接使用分两步才能跑通：
    1. **解锁**：读 `MF_TRANSFORM_ASYNC` 判定；异步时配置媒体类型前置 `MF_TRANSFORM_ASYNC_UNLOCK=TRUE`，否则首次 `SetInputType` 报 `MF_E_TRANSFORM_ASYNC_LOCKED (0xC00D6D77)`。
    2. **事件模型**：解锁只是入场券——此后必须 QI `IMFMediaEventGenerator` 按 MS「Asynchronous MFTs」驱动：`MFT_MESSAGE_NOTIFY_BEGIN_STREAMING` + `START_OF_STREAM` 开头流；输入只在收到 `METransformNeedInput` 后 `ProcessInput`（未请求的帧直接丢弃，实时语义），输出只在收到 `METransformHaveOutput` 后 `ProcessOutput`（其余时机调用返回 `E_UNEXPECTED (0x8000FFFF)`）。flush / D3D 互操作运行期回退后事件队列作废，须等新的 NeedInput 再恢复送帧。
    以上两步缺一都表现为「会话连上但画面永远不出」。同步 MFT 无 `MF_TRANSFORM_ASYNC` 属性，走原有同步泵，行为不变。回归由 `crates/mode-cua-ax/tests/rd_media.rs` 的 `hardware_h264_encoder_builds_with_async_unlock` 兜住（无硬件自适应环境跳过）；真实出流由本机探针验证（动画窗口下 ~22fps 连续帧、事件模型稳态每帧配对）。
  - **降级预案**：D3D11↔MF 互操作在目标环境不可用时，改为 CPU 拷回 + GDI 光标合成 + BGRA→NV12 转换后走系统内存输入（1080p60 预算内），协议与上层零改动。
- **帧循环**：目标 60fps（跟随显示刷新率）；DUPL `ACCESS_LOST`（锁屏/安全桌面/会话切换）→ 状态事件 → 信令 `freeze`，恢复后 `unfreeze` 并强制 IDR。
- **napi 导出**：`rdMediaStart(opts)`（含帧回调 ThreadsafeFunction、状态回调）、`rdMediaSwitch(monitor)`、`rdMediaStop()`；纯逻辑（帧头打包、GOP 标记、IV 派生）保持可 `cargo test` 的独立函数。

## 主控 renderer

- `VideoDecoder` 配置 `avc1`（Annex-B 分片直喂，关键帧 flag 对应 `type:"key"`）；分辨率变化由新 SPS 触发 `configure` 重配。
- 渲染：解码完成即画 canvas，**只保留最新帧**（绘制忙时丢弃旧帧）——自建零缓冲替代抖动缓冲；`cursor-none` 语义不变（光标已烧进画面）。
- 状态条：fps = 实际绘制帧率；延迟 = ping/pong RTT；分辨率 = decoder 配置尺寸（沿用既有显示格式）；
  端到端帧龄 = 采集时刻→本端绘制的 window 中位数（时钟偏移由 pong 标定）；码率 = 近一拍收流字节折算
  （kbps）；解码队列门控丢帧数 > 0 时显式提示（画质劣化的直接解释，无丢帧不占位）。解码队列长度与
  冻结次数在采集器中累计（P2-4 面板指标，供测试与后续调试读数）。
- `RemoteDesktopSession` 的 `<video>` 替换为 `<canvas>`；`InputCapture`、坐标映射、letterbox、全屏、操作栏零改动。

## 事件顺序

```text
① 会话请求与同意（POST）：主控 host 代发（设备令牌，携 `?quality=` 与 `?codec=`
   （h264|hevc，主控按本地解码能力预检给出，缺省/旧主控=h264））→ 被控 host 校验令牌/能力位
   → 被控 main 同意（已记住直通；拒绝/超时 → 稳定错误码）→ 信令 ticket 与媒体 mediaTicket
   随 POST 响应一并签发（主 spec 事件顺序 ② 不变；被控 host 记 `sessionCodec`，
   不需要把 codec 回传主控——解码串由码流自描述派生）
② 同意通过（被控 coordinator）：生成 key/salt → MediaStart（broker
   rd_media_start{monitor, endpoint, key, salt, codec}）→ helper 连中继口、起 DUPL+MF
   （主控 ready 前只采集不推流）
③ 信令（WS attach 后）：主控 hello(v2) → 被控 monitors → 主控 select-monitor
   → 被控回 media-open{key,salt}（首个 select-monitor 的应答；media-open 不得早于 WS
   attach 下发——同意时刻信令 WS 尚未建立，会被被控 host 按无会话连接丢弃，主控将永远
   拿不到 key/salt、媒体 WS 永不建立）
   → 主控 renderer 连 media WS（mediaTicket；mediaUrl 已经 rdStartSession 直达）
   → 上行 {t:"ready"} → helper 开始推流：帧 → 加密 → 管道 → 中继 → WS → renderer 解密 → 解码 → 画
   select-monitor 是**会话级**握手（2026-10-13 修复）：media-open 的 key/salt 逐会话生成、
   不跨会话复用——会话重建（改档、断线全会话重连）后新会话的 monitors 到达时，主控必须
   **重发一次 select-monitor**（即便用户选择的显示器未变），否则被控不会下发新会话的
   media-open，主控媒体面永不重建（画面冻结在旧帧、输入失效）。同会话内重复 monitors
   仍不重发（状态机以「会话级欠账」标记区分两种场景）。
④ 输入：InputCapture（即时发送，不在主控排队）→ media WS 上行 → host **同 tick 合批**
   （pointer 0ms 聚合、control 即时 flush 且先出已缓冲的 pointer 保持同帧先后）→ **逐帧
   串行**注入 broker raw_input（同一时刻至多一帧在途，保证绝对坐标事件不乱序——乱序会
   表现为光标回跳）→ helper 注入（既有）
   RTT：ping → host 即回 pong（不经 helper）
⑤ 锁屏：DUPL ACCESS_LOST → 状态事件 → 信令 freeze → renderer 显示既有冻结提示；
   恢复 → unfreeze + 强制 IDR
⑥ 结束（任一端 bye / 点结束 / 关开关 / 退出）：
   rdMediaStop（释放 DUPL 与 MF）+ 关中继口 + media WS 断 + 两张票据各自失效
```

## 失败语义

| 失败 | 归因与行为 |
| --- | --- |
| 旧对端缺 `remoteDesktopNative` / hello 版本不符 | `unsupported`：设备页置灰文案或会话内错误展示，**不回退 WebRTC** |
| helper 未拉起 / DUPL 或 MF 编码器创建失败 | `unsupported`（能力面缺失，不自动重试；重拉 Helper 走既有按需拉起与退避） |
| 媒体 WS 瞬断（信令存活） | **自动重连（P2-1）**：renderer 经信令 `{type:"request-media-ticket"}` 重签媒体票（同 clientId、单次消费、30s；被控在会话 WS 上本地回帖 `media-ticket`，不落信令转发面）→ 重连媒体 WS 续流。断流期间中继丢帧（不缓冲）；重连后只放行关键帧 + 首个关键帧强制重配解码器（防花屏残留）+ 立即请 IDR。退避 250ms→2s、8 次上限，耗尽或本端已收口则按压媒失败收口 |
| 整会话传输断开（信令 WS 断，非 bye/本地结束） | **自动重连（P2-1）**：main 推 `closed{retryable}` → renderer 退避重走 `POST /api/remote-desktop/session`（新票据/新 key/salt/mediaUrl 全量重建）；被控对 **90s 窗口内同 clientId** 重连复用最近一次「用户允许」（不二次弹窗；总开关/单会话互斥仍逐次校验）。busy/internal 可重试，明确拒绝（disabled/consent_denied 等）立即按错误码收口 |
| 锁屏 / 安全桌面 | 非错误：`freeze` 冻结提示，恢复自动续流（既有语义） |
| 中继端口异常 / 帧解密失败 | 计入 warn 日志（不含密钥与帧内容）；连续失败按媒体断开收口 |
| DUPL 零指针投递（软件光标/虚拟显示环境） | 非错误：切 user32 兜底采样继续合成光标（见「光标」节）；user32 也拿不到形状（光标隐藏/调用失败）→ 该帧不画光标，**画面与会话不受影响**（光标缺席不是会话级失败） |
| helper 侧 `[rd_media]` 运行告警（GPU 合成回退、形状渲染失败等） | 必须**接入主日志**（warn 级）——此前 helper stderr 无消费者，运行期回退/降级在日志里完全不可见（2026-10-13 排查实证：日志零条 `[rd_media]`） |

## 删除与保留清单

**删除**（P4 一次清理，含其专有测试）：`capturePage.ts`、`captureWindow.ts`（`listRemoteDesktopMonitors` 抽出到独立模块保留）、`codecPreferences.ts`、`useRemoteDesktopSession` 中 RTCPeerConnection/DataChannel/ontrack、`RemoteDesktopSession` 的 `<video>`、信号 schema 的 `offer/answer/ice`、隐藏采集窗生命周期（coordinator 的 start-capture 改接 `rd_media_start`）。

**保留**：同意/指示/设置/能力位/票据（扩第二张）/显示器枚举/注入/kill-switch/日志与 i18n 纪律；`waitIceGathering` 等 WebRTC 专属逻辑随删。

## 安全

- 媒体加密：AES-256-GCM 逐帧（承接主 spec「媒体强制加密」，实现从 DTLS 换为逐帧 GCM）；IV = 盐∥seq，每会话独立 key 保证不重复。
- 密钥分发只走已鉴权信令链；host/中继不可读明文；密钥与令牌不进日志（既有纪律）。
- media WS = 票据单次消费（30s、绑定 clientId）+ 连接后仅凭 key 解密——票据证明会话归属，key 保证机密性。
- kill-switch / 被控总开关在 `rd_media_start` 前的既有准入不变。

## 验收场景（两台 Windows 实机，用户执行）

- [ ] 连接 → 同意 → 画面出现；状态条 fps≈60、RTT 读数 <20ms；主观跟手明显优于 WebRTC 版（记录端到端主观/秒表数据）。
- [ ] 对端光标在画面内随动；本地光标仍在画面区隐藏。
- [ ] 锁屏 → 冻结提示 → 解锁恢复续流，无重连步骤。
- [ ] 会话中切换显示器：不重新弹同意，画面切换 <1s（含强制 IDR）。
- [ ] 输入全套（三键/拖拽/滚轮/快捷键/中文文本）在对端生效；操作栏/状态条本地可点（回归上轮修复）。
- [ ] 断开 / 被控点「结束」/ 关总开关 / 退出应用 → 双端收口，无残留（DUPL 释放、中继口关闭、票据失效，进程与端口核查）。
- [ ] 旧版本对端：设备页显示不支持文案，不发起媒体。
- [ ] 断线恢复（P2-1）：会话中拔插网线/切 Wi-Fi → 状态条显示「正在恢复」→ 恢复 <3s（退避 250ms 档）且画面无花屏残留；60s 内重连不重新弹同意窗；本端主动断开/对端 bye 不触发自动重连。
- [ ] 抓包（LAN 口）只见密文；中继口日志无明文帧与密钥。
- [ ] kill-switch 开 → 能力位 false，入口即挡。

## 已知限制

- 混合 DPI（多屏 scaleFactor 不一致）：输入坐标按「DIP bounds × scaleFactor → 物理 rect」近似换算，单屏正确、多屏跨缩放存在偏差（沿用一期已知限制口径；本方案未解决）。

## 测试策略

- **Rust**（`cargo test`）：帧头/flags 打包、IV=盐∥seq 派生、GOP/IDR 标记状态机、DUPL 状态事件纯逻辑（模拟 ACCESS_LOST 序列）；**光标语义（2026-10-13）**：`render_cursor_shape` 三类型断言（COLOR 保留真 alpha 且 `visible=alpha>0`、MASKED_COLOR 按 ARGB-alpha-掩码布局且 0xFF 打 XOR 标记、MONOCHROME 不变）、CPU 补丁混合纯函数（真 alpha 混合 / XOR(bg^color) / 替换 / 保留背景四分支）；**HEVC key 检测（2026-10-13）**：`contains_hevc_idr` 对 BLA/IDR/CRA NAL type 16..21 判真、TRAIL 判假，与 H.264 `contains_idr` 互不串扰。
- **Node**（仓库既有 tsx --test 先例）：中继长度前缀编解码、双票据各自单次消费、media-open 下发序列、pong 即答、密文透传（不解密断言）。
- **renderer**：解码喂帧状态机（mock VideoDecoder：SPS 重配、丢旧帧、关键帧 flag）、输入 sink 走 WS、状态条数据源切换；**codec 串派生（2026-10-13）**：`codecStringFromAnnexB` 对合成 H.264 SPS 出 `avc1.*`、对合成 HEVC VPS+SPS 出 `hev1.*`（profile/兼容位/tier/level/约束字节向量锁定），未知流返回 null。
- **回归**：全仓四包测试 + `pnpm typecheck` / `lint` / 架构检查 + `pnpm build:cua-helper` 构建通过。
- **不在本环境**：双机实机对拍（验收清单交用户）。

## 里程碑（供 writing-plans 拆分）

1. **P1 采集与编码**：`rd_media` 模块 + napi 导出 + cargo 单测（不上线）。
2. **P2 传输与解码**：双票据、中继、加密、media-open、renderer 解码画布（两端内部可通流）。
3. **P3 输入与信令 v2**：输入迁移、freeze/unfreeze、版本门、状态条切换。
4. **P4 直替清理**：删除 WebRTC 面、全量回归、打包构建、验收清单交付。

## 实现进度

- [x] P1 采集与编码（rd_media + napi + cargo 单测）
- [x] P2 传输链（双票据/media WS/中继/helper 泵/被控接线）
- [x] P3 renderer 切换（canvas 解码/输入/状态条/版本门/冻结 UI）
- [x] P4 直替清理（WebRTC 面全删；2026-10-12）
- [ ] 两台 Windows 实机验收（见「验收场景」，由用户执行）
