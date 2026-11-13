# Spec：远程桌面自适应码率与画质降档（Rust `remote-desktop-streaming`）

状态：2026-11-03 第一版（`ui/remote-desktop/adaptiveBitrate.ts` 纯状态机子集）；
2026-11-04 第二版追加会话状态条统计采集器（`ui/remote-desktop/sessionStats.ts`）。
真源：`packages/ui/src/remote-desktop/adaptiveBitrate.ts`、`packages/ui/src/remote-desktop/sessionStats.ts`。

## 1. 范围

| Rust 入口 | TS 对应 |
| --- | --- |
| `initial_adaptive_bitrate_state` | `initialAdaptiveBitrateState` |
| `step_adaptive_bitrate` | `stepAdaptiveBitrate` |
| `next_lower_quality` | `nextLowerQuality` |
| `step_quality_downshift` | `stepQualityDownshift` |
| `SessionStatsCollector`（八个 `on_*` + `sample`） | `createNativeSessionStatsCollector`（八个 `on*` + `sample()`） |
| `ADAPTIVE_BITRATE_HIGH/LOW`、`DOWNSHIFT_*` | 同名常量 |

只迁移判定状态机与统计采集器。媒体 WebSocket 下发 `set-bitrate`、`switchQuality`
重建会话与 React 生命周期不迁；`nowMs`、RTT、丢帧由调用方注入（采集器的时钟改为
`sample(now_ms)` 传参，对应 TS 构造时注入的 `now`，可观测行为一致）。

## 2. 行为

### 2.1 两档码率

- 初始为高档；`lowBps` 取 `min(low, high)`，畸形入参不反转升降档语义。
- 拥塞 = `rttMs !== null && rttMs > 60` 或 `droppedFrames > 0`。
- 拥塞时：已在低档只清零 `healthyStreak`；否则立即降到低档，记
  `lastSwitchAt = nowMs`，`nextBps = lowBps`。
- 非拥塞：健康 = `rttMs !== null && rttMs <= 30`；健康则 `healthyStreak + 1`，
  否则清零。低档下连续健康 ≥ 4 次且距上次切换 ≥ 10s（`lastSwitchAt === null`
  视为已满足）时升档，记 `lastSwitchAt = nowMs`、`nextBps = highBps`、streak 归零。
- `rttMs === null` 既不算健康也不算拥塞；`droppedFrames` 非零即拥塞。

### 2.2 画质降档

- 降档链：`sharp → balanced → smooth → null`。
- 拥塞 = `droppedFrames >= 3 && rttMs !== null && rttMs >= 120`（两个维度同时超阈）。
- 不拥塞：`congestedStreak` 清零，保留 `lastDownshiftAt`。
- 拥塞：streak 累计；连续 ≥ 2 且冷却满足（`lastDownshiftAt === null` 或
  `nowMs - lastDownshiftAt >= 30_000`）时返回 `downshift = true`，并把状态复位为
  `{congestedStreak: 0, lastDownshiftAt: nowMs}`；否则保留 streak、`downshift = false`。

### 2.3 会话状态条统计采集器

状态条读数全部来自自有事件计数（旧 WebRTC `getStats` 已移除）：

| 读数 | 数据源 | 采样口径 |
| --- | --- | --- |
| `fps` | `on_decoded` 每帧一次 | 采样窗内解码帧数 ÷ 实际经过秒数（四舍五入），采样后清零；首窗从采集器创建时刻起算；无解码返回 0（非 null，「画面未连」由上层区分） |
| `rttMs` | `on_pong`（RTT = 收到时刻 − ping 时间戳，调用方算好） | 取最近一次 |
| `frameAgeMs` | `on_frame_age` 每帧一次 | 窗口内中位数（奇数取中位；偶数取中位两数均值四舍五入），采样后清窗 |
| `width` / `height` | `on_configured`（首帧/换 SPS） | 取最近一次 |
| `bitrateKbps` | `on_bytes_received` 每条下行帧 | `bytes × 8 / 间隔ms` 四舍五入；首拍（无间隔基准）null，按拍清零 |
| `decodeDrops` | `on_frame_dropped`（解码队列门控丢 delta） | 间隔计数，采样后清零 |
| `decodeQueue` | `on_decode_queue` 每个被接受帧 | 取最近一次（「链路是否在追上」的即时读数） |
| `freezeCount` | `on_frozen`（锁屏/失去访问） | 会话累计，跨采样不清零 |

`sample(now_ms)` 有副作用（清零计数与窗口），只能由唯一的轮询消费者按固定节拍调用。
`fps` 是**每秒帧率**，不是窗口内计数——2026-11-13 修正：旧口径把 3 秒计数当 fps 展示，
读数放大 3 倍（真机 80 实际约 27）。

## 3. 已知差异

1. 时钟与采样完全注入，Rust 侧不读系统时间；生产调用方负责 `Date.now()` 口径。
   采集器把 TS 的构造注入 `now` 改成 `sample(now_ms)` 传参。
2. 状态用强类型结构而非 `number`；`RemoteDesktopQuality` 只接受三个合法值，
   TS 的 `default` 分支（未知字符串）在 Rust 由解析失败显式拒绝。
3. 不包含 React hook / 媒体通道副作用；状态条空态常量
   `EMPTY_REMOTE_DESKTOP_SESSION_STATS` 属渲染层展示默认值，不随采集器迁移。
4. JSON 输出与 TS 对齐：整数值发整数（`17` 而非 `17.0`），小数原样保留。

## 4. 验收

- `pnpm remote-desktop-streaming:fixtures` →
  `crates/remote-desktop-streaming/fixtures/adaptive.json` 与
  `crates/remote-desktop-streaming/fixtures/session-stats.json`
  - 初始状态（默认、自定义、low 高于 high）；
  - 两档码率降档、低档重复拥塞、RTT 边界 60/61、健康边界 30/31、null RTT、
    恢复 streak/驻留/无 `lastSwitchAt`；
  - 画质降档链、丢帧/RTT 阈值边界、连续拥塞、冷却期内与冷却后；
  - 会话统计 11 条事件序列：fps 清零、首拍 0、RTT/分辨率取最近、空态、帧龄
    奇偶中位数与 .5 进位、换分辨率、码率首拍 null/正常/清零、丢帧清零、队列
    取最近、冻结累计、混合窗口（小数 RTT/队列、码率 .5 进位）。
- `cargo test -p remote-desktop-streaming`（TS 回放 + 状态机/采集器单测）。
- `cargo clippy -p remote-desktop-streaming --all-targets -- -D warnings`。
