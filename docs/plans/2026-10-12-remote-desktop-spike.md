# Windows 远程桌面本机 spike 记录（2026-10-12）

状态：本机部分已完成，两机清单待执行。关联 spec：[lan-remote-desktop.md](../specs/lan-remote-desktop.md)（草案，待拍板）。

## 结论摘要

三个技术未知全部消掉，spec 的设计前提成立：① 隐藏渲染器可持续采集屏幕并推 WebRTC（27.5fps@1080p）；② 关闭 mDNS 候选混淆后 ICE 候选全部为裸 IP；③ 注入管线延迟 p50 2–4ms、中文文本逐字精确，且实测发现现 `perform` 路径强制前台——印证 spec「`raw_input` 不走 CUA 动作语义、无前台要求」的必要性。剩余不确定性只有"真实网络一行"（跨机延迟/防火墙），由下面的两机清单覆盖。

## 环境

Windows 10 19044 x64，单显示器 1920x1080 @100% 缩放，Node v24.18.1，Electron 41.0.3（仓库 node_modules），cua-helper 运行件在位（`packages/mode-cua/dist-cua-helper/`）。

## S1 本机环回：Electron 双窗 WebRTC 采集（通过）

harness：`.spike/remote-desktop/main-webrtc.mjs`（git 未跟踪）。

| 项 | 实测值 |
| --- | --- |
| 隐藏窗采集（`show:false` + `backgroundThrottling:false`） | 成功，1920x1080@30，`cursor:"always"`（光标随帧） |
| 连接建立（handler 注册→offer→answer→connected） | 约 3.5s（含采集启动与 ICE 收集） |
| 帧率 | 27.5 fps（4s 采样，静态桌面）；inbound `framesPerSecond`=29 |
| 编码协商 | **video/VP8**（1080p 静态画面约 1.4 Mbps） |
| ICE RTT | ~1ms（同机环回） |
| DataChannel | open（输入通道承载能力确认） |
| mDNS | **0 个 `.local` 候选**；候选为裸 IP：192.168.1.4 / Tailscale 100.90.96.50 / 公网 IPv6 2409:… |
| 轨道设置 | `resizeMode:"crop-and-scale"`、`logicalSurface:true` |

要点：

- 隐藏窗采集可行——spec 的隐藏渲染器设计成立；`backgroundThrottling:false` 必须开，否则隐藏窗被节流。
- `--disable-features=WebRtcHideLocalIpsWithMdns` 生效，候选为裸 IP（跨机可达性仍需两机清单确认）。
- 默认协商到 VP8；要 H.264（Windows 硬件编码）需实现时显式 `setCodecPreferences`。
- WebRTC 会把本机全部地址（含 Tailscale/公网 IPv6）作为候选带出——对端是配对设备可接受，记为隐私面说明。

## S2 Helper 直连注入（通过）

harness：`.spike/remote-desktop/main-inject.mjs`（fork `dist-cua-helper` + broker 行协议直连，模拟远控输入管线的注入端）。

| 项 | 实测值 |
| --- | --- |
| Helper 直连（不经 Mode 全栈） | fork→ready→health（pid 对齐）；`screen_probe:{locked:false}` |
| 单击落点（绝对坐标 SendInput） | 8.15ms 往返；光标落点偏差 **0px** |
| 文本注入（含中文） | `"Mode远程桌面spike 中文123 abc"` 逐字精确（KEYEVENTF_UNICODE） |
| click 延迟（60 次） | p50 1.93ms / p95 3.84ms / max 26.35ms |
| type_text 延迟（30 次） | p50 3.6ms / p95 4.2ms |
| capture 单帧（560x360） | 23ms，PNG 11KB |
| 鼠标位置恢复 | 精确还原到运行前坐标 |

要点：

- 注入延迟远低于高频输入预算（鼠标 100Hz = 10ms/事件），"主控→IPC→helper→SendInput"管线余量充足，输入通路无需单独优化。
- **现 `perform` 路径强制目标窗口前台**（首次调用返回 `foreground_required`、零下发），且带 UIA 动作语义。远程桌面的注入目标是"对端当前前台窗口"（由对端实际使用状态决定），不能复用该校验——实装必须新增 `raw_input` 面（Rust 原语已在 `input.rs`）。
- 中文文本注入逐字精确；混合 DPI/多屏坐标留待两机清单补充（本机为单屏 100%）。

## 未覆盖（交由两机清单或实装阶段）

真实网络延迟与丢包、Windows 防火墙行为、混合 DPI 多屏、锁屏/UIPI 边角、5 分钟以上稳定性、Tailscale 跨网段路径。

## 两机验证清单（待执行）

### 准备

- 两台 Windows（x64），同一局域网或 Tailscale 内；
- 把 `.spike/remote-desktop/`（`page.html` + `main-webrtc-pair.mjs`）拷到两台机器；用仓库的 Electron 二进制运行（或把整个仓库同步过去）。

### 步骤（约 10 分钟）

1. 机器 A：`node_modules/electron/dist/electron.exe main-webrtc-pair.mjs --role=offer --clock`
   （如弹 Windows 防火墙提示 → 允许，并记录是否弹出）。
2. 复制 A 终端整行 `SDP_OFFER:<base64>`。
3. 机器 B：`…electron.exe main-webrtc-pair.mjs --role=answer --offer=<base64>`。
4. 复制 B 终端 `SDP_ANSWER:<base64>`，粘贴回 A 终端并回车。
5. B 窗口出现 A 的屏幕（含 A 端大号毫秒时钟），右上角叠 B 本机毫秒时钟；两侧每 5 秒打印一条 `STATS:`。
6. 对 B 窗口截图：视频内时钟（A 屏幕内容）与叠层时钟（B 本机）的毫秒差 ≈ 单向玻璃到玻璃延迟。

### 判定标准

| 项 | 通过线 |
| --- | --- |
| 连接建立（自粘贴起） | 同网段 ≤5s；Tailscale ≤15s |
| 帧率（静态桌面 1080p） | ≥25 fps，5 分钟不掉 |
| ICE RTT | 同网段 <10ms；Tailscale <100ms |
| 丢包 | `packetsLost` 不持续增长 |
| 玻璃到玻璃延迟（时钟法） | 同网段目标 <100ms（理想 <50ms） |
| 稳定性 | 连续 5 分钟无断连、fps 不归零 |

### 排障

- 连不上：确认两侧防火墙放行；确认同一网段（AP 隔离会阻断）；Tailscale 场景确认双方 `tailscale status` 在线。
- fps=0：A 端锁屏/息屏会冻结画面；确认没有误关 `backgroundThrottling`。
- 延迟异常大：可能走了 Tailscale 中继而非直连（RTT 会显著变大），看 `STATS` 的 rtt 与候选对端地址。

## 对 spec 的反馈（已回写）

- 「Rust/Tauri 迁移兼容性」：mDNS 一条由"实现时验证"更新为"本机已验证候选为裸 IP"；编码一条补"默认协商 VP8，如需 H.264 需 `setCodecPreferences`"。
- 「接口变更」被控 main 一条补"隐藏窗采集本机已验证"。
- 「验收场景」新增一条：注入目标 = 对端当前前台窗口，不得复用 `perform` 的前台校验（spike 实测发现）。

## 复现

- harness：`.spike/remote-desktop/`（git 未跟踪，不入库；如需入库应迁移为 TS 或走 `scripts/language-policy-allowlist.txt` 并经 spec 批准）。
- 本机复跑：`cd .spike/remote-desktop && ../../node_modules/electron/dist/electron.exe main-webrtc.mjs`（注入侧为 `main-inject.mjs`；运行期间鼠标被短暂移动并自动恢复）。
