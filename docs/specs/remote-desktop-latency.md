# Spec：远程桌面端到端延迟治理（2026-11-13）

状态：已实现（B 档全部 + C 档有界队列/背压部分；C 档「独立 UDP/RTP 媒体面」仍是后续项）。
真源：`crates/mode-cua-ax/src/rd_media/`、`packages/ui/src/remote-desktop/`、
`packages/desktop/src/host/remoteDesktopMediaRelay.ts`、`packages/desktop/src/host/remoteV3/`、
`packages/remote-core/src/app/`。

## 1. 背景与实测

真机会话状态条（清晰档 1920×1080）读到：`80 fps / 11.7 Mbps / RTT 293ms / 帧龄 ~240ms / 丢帧 8`。
按当时的代码口径，`fps` 是 **3 秒采样窗内的解码帧数**（不是每秒帧率），所以真实解码约
27fps；`11.7 Mbps` 已超过清晰档 8Mbps；`293ms` 的媒体 ping/pong 与视频帧排在同一条
可靠有序队列里，读数含排队时间。结论：**主要矛盾是排队与帧率/码率失配，而不是单纯物理 RTT**。

链路实际形状（v3）：

```text
被控 helper 采集(DXGI) → MF 编码 → helper AES-GCM → 本机 TCP → 被控 host 中继
  → 被控 LAN WS → v3 wsTunnel → v3 StreamMux(JSON+base64) → Noise 分片(48KB)
  → 数据面(lan/p2p/relay，可靠有序；relay 再 base64 一次)
  → 主控 v3 解复用 → rdProxy 本地 WS → renderer 解密 → VideoDecoder → canvas
```

每一跳都没有「丢旧帧」的上界：链路一变慢，帧就在内存里排队，延迟线性增长；丢一帧又触发
IDR 请求，IDR 体积大进一步抬高峰值，形成正反馈。

## 2. 产品规则（本次新增）

1. **生产者限速**：采集帧率的上界 = 会话预设 `fps`（清晰/均衡 60、流畅 30），不再跟随
   显示器刷新率。运行期可经 `{t:"set-bitrate", bps, fps?}` 把有效帧率降到预设以下
   （最小 15fps），恢复按滞回回升，最高回到预设。
2. **只在生产者侧跳过**：任何「跟不上」都优先降低被控采集/编码帧率或码率；
   **不在解码器入口丢已编码 delta**——丢 delta 会破坏参考帧链，只能靠 IDR 修复，代价远高于限速。
3. **有界队列**：v3 媒体路径上每个跨进程/跨网络出口都必须有明确的队列上界；超过上界时
   丢**最旧的二进制视频帧**（文本控制帧与输入事件永不丢）。
   - 被控 `wsTunnel`：`stream.bufferedAmount > 128KB` 时丢二进制帧。
   - 主控 `rdProxy`：本地 renderer WS `bufferedAmount > 128KB` 时丢二进制帧。
   - 被控媒体中继的本地 WS 128KB 背压线保留（LAN 直连路径仍有效）。
4. **重同步而不是 IDR 风暴**：
   - renderer 用 `seq` 检测缺口（`seq > lastSubmittedSeq + 1`）或队列溢出时进入
     `resync`：只放行关键帧，其余 delta 丢弃并计数；
   - 关键帧请求去抖 **2 秒**（renderer）/ **1.5 秒**（被控中继），不再按 300/500ms 刷 IDR；
   - 重连后的首个关键帧仍强制重配解码器（既有规则不变）。
5. **编码器按 CBR 收口**：`AVEncCommonRateControlMode=CBR`、`MaxBitRate=MeanBitRate`、
   `BufferSize≈200ms`、`MaxNumRefFrame=1`、`B` 帧=0、`AVLowLatencyMode=TRUE`。
   `set-bitrate` 同时更新 Mean/Max/Buffer，避免复杂画面把链路打成突发。
6. **读数必须可信**：
   - `fps` = 采样窗内解码帧数 / 实际经过秒数（四舍五入）；首个窗口按采集器创建时刻起算。
   - 状态条显示传输方式（lan / p2p / relay），以及综合丢帧数（含重同步丢弃）。
7. **HEVC 必须硬解优先**：`probeHevcDecodeSupport` 带 `hardwareAcceleration:"prefer-hardware"`；
   不满足就回 H.264（软解 1080p 会把解码端直接压到 ~27fps）。

## 3. 状态所有者与事件顺序

| 状态 | 所有者 | 说明 |
| --- | --- | --- |
| 采集帧率节拍 | 被控 `rd_media` worker（`FramePacer`） | `rd_media_set_fps` 写 pending，worker 下一 tick 消费 |
| 编码码率/码控 | 被控 `Encoder`（`ICodecAPI`） | `set-bitrate` 经中继 kind4 到达 |
| 传输队列水位 | `RemoteSocket.bufferedAmount` → `RemoteStream.bufferedAmount` | p2p/werift 与 WS 各自实现 |
| 丢帧/重同步 | 主控 renderer `nativeMediaSession` | 唯一判决点；中继只做背压丢弃 |
| 生产者降帧 | 主控 renderer `producerRate` | 由丢帧/RTT 采样驱动，经 `set-bitrate` 下发 |
| 传输方式 | 主控 v3 client 连接 | 会话建立结果带回 renderer，只读展示 |

```text
采样窗(3s) 结束
  → renderer 统计 fps/RTT/丢帧
  → producerRate: 解码压力持续 → 下调 fps（最多每 10s 一档，最低 15）
  → {t:"set-bitrate", bps, fps}
  → 被控媒体中继 kind4(len=9) → helper → napi PENDING_FPS/PENDING_BITRATE
  → worker 下一 tick 应用 FramePacer 与 ICodecAPI

链路拥塞
  → wsTunnel/rdProxy 发现 bufferedAmount>128KB
  → 丢最旧二进制帧（文本不丢）
  → renderer 发现 seq 缺口 → resync（只等 key）+ need-key(2s 去抖)
  → 被控出 IDR → renderer 重配/恢复
```

## 4. 验收

- `80 fps` 类读数消失：状态条 fps 与真实解码帧率一致（3 秒窗口内 60 帧显示 20，而非 60）。
- 静屏或正常桌面：清晰档 1080p 下采集帧率不超过预设；链路拥塞时帧龄不再随会话时长单调上涨。
- 丢帧后 ≤1 个 GOP（或一次 IDR 往返）内恢复，不出现「越丢越卡」的 IDR 风暴。
- 状态条能看到传输方式；`wsTunnel/rdProxy` 的丢弃可在 renderer 丢帧数上体现。
- 单测：
  - `FramePacer`（Rust）：节拍、改 fps、不累积欠账；
  - `producerRate`（TS）：降档/回升/驻留；
  - `sessionStats`（TS/Rust）：fps 按经过时间折算、首窗、清零；
  - `nativeMediaSession`：seq 缺口 → resync、只等 key、IDR 去抖；
  - 中继：kind4 len=9 解析、fps 透传、IDR 去抖；
  - `wsTunnel/rdProxy`：超水位只丢二进制帧。
- `pnpm typecheck` / `pnpm lint` / `cargo test -p mode-cua-ax` / `cargo test -p remote-desktop-streaming`。
