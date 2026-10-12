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
  renderer：media-open{url,key}（经既有信令链下发）→ 连 media WS
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

## 协议

### 信令 union（protocolVersion 2）

- **保留**：`hello{protocolVersion:2}`、`monitors`、`select-monitor`、`bye`、`error`。
- **删除**：`offer`、`answer`、`ice`（WebRTC 消费者随媒体面移除）。
- **新增**：
  - `media-open{url, key, salt}`（被控 → 主控，同意后发出）：`url` = 对端 LAN server 的 media WS 地址，`key`/`salt` 十六进制；
  - `freeze{reason:"locked"|"access-lost"}` / `unfreeze`（被控 → 主控，DUPL 丢失/恢复时），复用既有「锁屏画面冻结 + 提示，解锁恢复」语义与文案。
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

## 被控 Rust（`crates/mode-cua-ax/src/rd_media/`）

- **采集**：DXGI Desktop Duplication（`windows` crate，与 `screen.rs` 同依赖面）；输出分辨率 = 显示器原生（>1080p 时由编码链缩放，见下）。
- **光标**：DUPL `PointerPosition` + `PointerShape` 合成进帧——指针形状纹理 + 位置画 quad（GPU 路线）；**降级预案**：GPU 合成受阻时改为「光标包围盒 CPU 合成」，不改对外协议。
- **编码**：MF 视频编码器（`MFT_ENUM_FLAG_HARDWARE` 选「H.264 HW」，Win10 系统自带）；输入 D3D11 纹理 → D3D11 Video Processor 出 NV12（承担 >1080p 缩放）→ MF `MFT_MESSAGE_SET_D3D_MANAGER` 全 GPU 路径。参数：平均码率 8Mbps、GOP 120 帧（≈2s@60fps）、`select-monitor` / 分辨率变化强制 IDR。
  - **降级预案**：D3D11↔MF 互操作在目标环境不可用时，改为 CPU 拷回 + GDI 光标合成 + BGRA→NV12 转换后走系统内存输入（1080p60 预算内），协议与上层零改动。
- **帧循环**：目标 60fps（跟随显示刷新率）；DUPL `ACCESS_LOST`（锁屏/安全桌面/会话切换）→ 状态事件 → 信令 `freeze`，恢复后 `unfreeze` 并强制 IDR。
- **napi 导出**：`rdMediaStart(opts)`（含帧回调 ThreadsafeFunction、状态回调）、`rdMediaSwitch(monitor)`、`rdMediaStop()`；纯逻辑（帧头打包、GOP 标记、IV 派生）保持可 `cargo test` 的独立函数。

## 主控 renderer

- `VideoDecoder` 配置 `avc1`（Annex-B 分片直喂，关键帧 flag 对应 `type:"key"`）；分辨率变化由新 SPS 触发 `configure` 重配。
- 渲染：解码完成即画 canvas，**只保留最新帧**（绘制忙时丢弃旧帧）——自建零缓冲替代抖动缓冲；`cursor-none` 语义不变（光标已烧进画面）。
- 状态条：fps = 实际绘制帧率；延迟 = ping/pong RTT；分辨率 = decoder 配置尺寸（沿用既有显示格式）。
- `RemoteDesktopSession` 的 `<video>` 替换为 `<canvas>`；`InputCapture`、坐标映射、letterbox、全屏、操作栏零改动。

## 事件顺序

```text
① 同意前（与现网一致）：hello(v2) → capability 门 → monitors → select-monitor
② 同意通过（被控 coordinator）：
   生成 key/salt → host：签发 mediaTicket（POST 响应内既有 ticket 字段旁新增）
   → 信令下行 media-open{url,key,salt}
   → broker rd_media_start{monitor, endpoint, key, salt} → helper 连中继口、起 DUPL+MF
③ 主控 renderer 收 media-open → 连 media WS（mediaTicket）→ 上行 {t:"ready"}
   → helper 开始推流：帧 → 加密 → 管道 → 中继 → WS → renderer 解密 → 解码 → 画
④ 输入：InputCapture → media WS 上行 → host → broker raw_input → helper 注入（既有）
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
| media WS 连不上 / 中途断开 | 会话收口，按既有 `closed.error` 文案回设备页；不自动重连 |
| 锁屏 / 安全桌面 | 非错误：`freeze` 冻结提示，恢复自动续流（既有语义） |
| 中继端口异常 / 帧解密失败 | 计入 warn 日志（不含密钥与帧内容）；连续失败按媒体断开收口 |

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
- [ ] 抓包（LAN 口）只见密文；中继口日志无明文帧与密钥。
- [ ] kill-switch 开 → 能力位 false，入口即挡。

## 测试策略

- **Rust**（`cargo test`）：帧头/flags 打包、IV=盐∥seq 派生、GOP/IDR 标记状态机、DUPL 状态事件纯逻辑（模拟 ACCESS_LOST 序列）。
- **Node**（仓库既有 tsx --test 先例）：中继长度前缀编解码、双票据各自单次消费、media-open 下发序列、pong 即答、密文透传（不解密断言）。
- **renderer**：解码喂帧状态机（mock VideoDecoder：SPS 重配、丢旧帧、关键帧 flag）、输入 sink 走 WS、状态条数据源切换。
- **回归**：全仓四包测试 + `pnpm typecheck` / `lint` / 架构检查 + `pnpm build:cua-helper` 构建通过。
- **不在本环境**：双机实机对拍（验收清单交用户）。

## 里程碑（供 writing-plans 拆分）

1. **P1 采集与编码**：`rd_media` 模块 + napi 导出 + cargo 单测（不上线）。
2. **P2 传输与解码**：双票据、中继、加密、media-open、renderer 解码画布（两端内部可通流）。
3. **P3 输入与信令 v2**：输入迁移、freeze/unfreeze、版本门、状态条切换。
4. **P4 直替清理**：删除 WebRTC 面、全量回归、打包构建、验收清单交付。
