# Spec：远程桌面自适应码率与画质降档（Rust `remote-desktop-streaming`）

状态：2026-11-03 第一版（`ui/remote-desktop/adaptiveBitrate.ts` 纯状态机子集）。
真源：`packages/ui/src/remote-desktop/adaptiveBitrate.ts`。

## 1. 范围

| Rust 入口 | TS 对应 |
| --- | --- |
| `initial_adaptive_bitrate_state` | `initialAdaptiveBitrateState` |
| `step_adaptive_bitrate` | `stepAdaptiveBitrate` |
| `next_lower_quality` | `nextLowerQuality` |
| `step_quality_downshift` | `stepQualityDownshift` |
| `ADAPTIVE_BITRATE_HIGH/LOW`、`DOWNSHIFT_*` | 同名常量 |

只迁移判定状态机。媒体 WebSocket 下发 `set-bitrate`、`switchQuality` 重建会话、
ping/pong 采样与 React 生命周期不迁；`nowMs`、RTT、丢帧由调用方注入。

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

## 3. 已知差异

1. 时钟与采样完全注入，Rust 侧不读系统时间；生产调用方负责 `Date.now()` 口径。
2. 状态用强类型结构而非 `number`；`RemoteDesktopQuality` 只接受三个合法值，
   TS 的 `default` 分支（未知字符串）在 Rust 由解析失败显式拒绝。
3. 不包含 React hook / 媒体通道副作用。

## 4. 验收

- `pnpm remote-desktop-streaming:fixtures` →
  `crates/remote-desktop-streaming/fixtures/adaptive.json`
  - 初始状态（默认、自定义、low 高于 high）；
  - 两档码率降档、低档重复拥塞、RTT 边界 60/61、健康边界 30/31、null RTT、
    恢复 streak/驻留/无 `lastSwitchAt`；
  - 画质降档链、丢帧/RTT 阈值边界、连续拥塞、冷却期内与冷却后。
- `cargo test -p remote-desktop-streaming`（TS 回放 + 状态机单测）。
- `cargo clippy -p remote-desktop-streaming --all-targets -- -D warnings`。
