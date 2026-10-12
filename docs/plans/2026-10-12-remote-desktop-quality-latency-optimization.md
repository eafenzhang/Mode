# 远程桌面「质量 / 延迟 / 稳定性」优化方案

- 日期: 2026-10-12
- 范围: LAN 远程桌面原生媒体通路(`lan-remote-desktop-native-media`),及对其有直接影响的信令/输入链路
- 目标: 端到端玻璃到玻璃 P95 < 100ms(验收下限),原生通路预算 25–40ms;画质与稳定性不劣化
- 前提: 本方案基于对当前代码的静态审阅(2026-10-12 夜间快照,T10 进行中)。所有位置引用的行为都需在实机验证清单上回归(Date: docs/specs/lan-remote-desktop-native-media.md §验收)

## 0. 延迟预算:瓶颈在哪

当前原生通路(理论上线链路)逐帧串行环节与估算:

| # | 环节 | 位置 | 现状 | 优化后 |
|---|------|------|------|--------|
| 1 | 取帧等待 | `crates/mode-cua-ax/src/rd_media/napi.rs:315` | ≤16ms(节拍固有) | 不变 |
| 2 | 桌面帧→proc 拷贝 | `capture.rs:352` | GPU 全帧拷贝 <1ms | 可省(见 P0-3) |
| 3 | **光标合成:回读+混合+回写** | `capture.rs:390-501` | **每帧 GPU→CPU 同步回读**(光标可见即触发),2–8ms | **0(GPU 合成)/ 跳过脏检查** |
| 4 | VP: BGRA→NV12+缩放 | `encode.rs:367-383` | GPU <1ms | 不变 |
| 5 | **NV12 回读 + CPU 拷贝进 MF** | `encode.rs:384-415` + `:418-470` | **每帧 GPU→CPU 同步回读 3MB + 再拷 3MB**,3–10ms | **0(D3D11↔MF 互操作)** |
| 6 | MF 硬编 | `encode.rs:226-266` | 5–15ms(硬件流水) | CODECAPI 低延迟档略降 |
| 7 | helper 加密+分帧+写 | `packages/mode-cua/helper/mediaPump.mjs:126-160` | ~0.3ms,3 次 Buffer 复制 | 减 1–2 次复制 |
| 8 | **本机 TCP(helper→relay)** | `mediaPump.mjs:64` | **Nagle 未关**,小包/尾包 0–40ms | `noDelay:true` |
| 9 | relay 分帧+转发 | `remoteDesktopMediaRelay.ts:92-112` | 每帧 1 次复制;4MB 才丢 | 去复制;阈值降到 128KB |
| 10 | **媒体 WS 传输** | `packages/server/src/http.ts:680-707` | LAN TCP;**ws 服务端 socket noDelay 未显式设置** | `setNoDelay(true)` + `perMessageDeflate:false` |
| 11 | **renderer 解密** | `packages/ui/src/remote-desktop/nativeMediaSession.ts:139-159, 204-219` | **每帧异步 `importKey`**,无解码队列门控 | 会话级 import;队列门控+丢帧请 IDR |
| 12 | 解码+画布呈现 | `nativeMediaSession.ts:76,100` | `drawImage`;canvas 未开 `desynchronized` | 开 `desynchronized`;只画最新帧 |

大项:3、5 两处**每帧 GPU→CPU 同步回读**串行存在,合计 5–15ms;11 每帧一个多余的异步跳;8/10 两处 Nagle 潜在 0–40ms;9 背压阈值 4MB(=8Mbps 下 4 秒)会把拥塞场景的延迟放大到秒级。

## P0 — 延迟主项(建议本轮 T13/T14 窗口内直接做)

### P0-1 编码输入去回读:走 D3D11↔MF 互操作(MFT_MESSAGE_SET_D3D_MANAGER)
- 位置: `crates/mode-cua-ax/src/rd_media/encode.rs` `blt_and_readback`(:367-415) / `feed_input`(:418-470)
- 现状: VP Blt → NV12 纹理 → `CopyResource` 到 staging → `Map` 同步回读 → CPU 逐行拷贝 → `MFCreateMemoryBuffer` + `Lock` 再拷一次 → `ProcessInput`。**两处全帧拷贝 + 一个 GPU 同步点/帧**。
- 改法(标准路径): `MFCreateDXGIDeviceManager` + `ResetDevice(d3d11_device)` → `mf.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, ...)`;输入样本用 `MFCreateDXGISurfaceBuffer` 直接包 VP 输出的 NV12 纹理(同 device),`AddBuffer` + `SetCurrentLength`。省掉回读与内存拷贝,编码器直接在 GPU 上取数据。
- 回退: `SetInputType/SetOutputType` 或 `SET_D3D_MANAGER` 任一步失败 → 保留现有系统内存路径(gpu_interop_enabled 开关,不可用时行为与今天完全一致,不影响验收下限)。
- 预期: -3~10ms/帧,并把采集线程从两个串行同步点里解放出来(消除卡顿尖刺)。
- 注意: 需保持输入纹理在 MFT 消费完之前存活(现结构同步 pump,天然满足);NV12 纹理可能需 `D3D11_BIND_RENDER_TARGET`+ 共享标志,按驱动实测调整。

### P0-2 光标合成去回读(GPU 合成,spec 主案)
- 位置: `capture.rs:390-501`(`build_cursor_patch` 的 ①回读 ②CPU 混合 ③回写)
- 现状: 光标可见的每一帧都做 `CopySubresourceRegion → Map(阻断)→ 逐像素混合 → UpdateSubresource`。
- 改法(二选一,按代价从低到高):
  a) **最小改动**:先加脏检查——`PointerPosition` 与形状缓存均未变化 → 跳过整个补丁(直接返回缓存的 `CursorPatch` 信息或 None)。静止场景省掉绝大部分回读。
  b) **GPU 合成(推荐)**:把光标形状渲染成小 BGRA 纹理(形状变化时才更新),`VideoProcessorBlt` 里作为第二路 input stream,配合 per-stream alpha 混合(`VideoProcessorSetStreamAlpha` + 双流 Blt)在 GPU 内合成;`proc` 纹理与回读整体删除。
- 同时修一个疑似功能问题: `capture.rs:341` 把 `LastPresentTime==0 && AccumulatedFrames==0` 全部按 `Idle` 丢弃——**纯光标移动(桌面无重绘)会走到这个分支,远端光标在静态画面上会不动**。修法: 该分支里检查 `PointerPosition/形状` 变化,有变化则走「用上一帧底图重新合成光标」的轻量路径(改动 a 下需保留一份无光标底图;改动 b 下重 Blt 一次即可)。实机必须验证: 远端画面上静置不动,本端拖动鼠标 → 对端光标应平滑随动。

### P0-3 采集帧直通 VP(顺手做)
- 位置: `capture.rs:352`(`CopyResource(proc, frame)`)
- 改法: P0-2b 落地后,VP 的输入 stream 直接用 `AcquireNextFrame` 拿到的帧纹理,在 `ReleaseFrame` 之前完成 Blt——整帧 `proc` 拷贝省掉。改动 b 之前不建议单独做(光标回写在 proc 上)。

### P0-4 renderer 解密/解码热路径修整(T13 整合)
- 位置: `nativeMediaSession.ts`
- 改法:
  1. `crypto.subtle.importKey` 移到会话建立时一次(现在 :209 每帧一次,60fps 下 60 次/秒异步导入 + 每帧多一个微任务跳转);
  2. 解码门控: `decoder.decodeQueueSize > 2` 时丢弃 delta 帧(不丢 key),并在发生丢弃后经 WS 上行发 `{t:"need-key"}`(见 P0-5)在途积压直接清空,只画最新帧;
  3. 到达序防护: 记 `lastSubmittedSeq`,`parsed.seq <= lastSubmittedSeq` 直接丢(解密是异步的,理论上可乱序完成);
  4. canvas 上下文加 `desynchronized: true`(:76),少一层合成器同步;`hardwareAcceleration: "prefer-hardware"`.
- 预期: -1~4ms 稳态,消除拥塞下的延迟堆积(当前 decode 队列无界 → 延迟无上界)。

### P0-5 背压与丢帧闭环:阈值下移 + IDR 请求通道(归入 T14)
- 位置: `remoteDesktopMediaRelay.ts:67, 92-112`;新增 napi 导出 + helper + 协议
- 现状: (a) 阈值 4MB ≈ 8 秒缓冲,到达前延迟已经烂掉;(b) 丢帧后**没有任何重新出关键帧的机制**,下一帧 delta 引用已丢数据 → 花屏直到下一个 GOP(≤2s)。
- 改法:
  1. `WS_BACKPRESSURE_BYTES` 降到 ~128KB(≈130ms@8Mbps);丢帧策略「超阈值丢 delta、保留 key」;
  2. 新增控制面: WS 上行 `{t:"need-key"}` → relay → helper(kind 3 管道消息)→ `rd_media_request_key`(napi 新增,内部调 `Encoder::force_idr`,该函数已存在 `encode.rs:220`);relay 自身发生背压丢弃时也走同一条链;
  3. renderer 侧 P0-4 的解码丢弃同样触发 `{t:"need-key"}`。
- 预期: 拥塞时延迟有上界,**丢帧后画面恢复从 ≤2s 缩到 ≤1 个 RTT + 编码延迟**;这是稳定性与体验的最大单项改善。

### P0-6 全链路 TCP_NODELAY(半天量级的小活,收益立竿见影)
- 仓库全局 grep:`setNoDelay` **零使用**。改动点:
  - `mediaPump.mjs:64` `connect({host, port, noDelay: true})`;
  - `remoteDesktopMediaRelay.ts:302` 收到 helper 连接后 `socket.setNoDelay(true)`;
  - 媒体 WS / 信令 WS 服务端连接: 在 `upgradeWebSocket` 的 `onOpen`(`http.ts:655-660, 684-690`)里对底层 socket `setNoDelay(true)`(ws 客户端默认已开,服务端连接需显式);
  - 顺带确认 ws 未开 `perMessageDeflate`(默认关闭,显式写死防回归)。
- 预期: 消除 Nagle×延迟 ACK 造成的 0–40ms 抖动尾巴,尤其影响小帧(键鼠输入 JSON、状态帧)与帧尾包。

## P1 — 画质与自适应

### P1-1 编码器档位与 CODECAPI(质量 +12~20%@同码率)
- 位置: `encode.rs:676-682`(profile)、`:235-245`(flush-IDR)、Bitrate 配置 `:665`
- 改法:
  1. `eAVEncH264VProfile_Base` → **Main**(+CABAC;显式 `CODECAPI_AVEncMPVDefaultBPictureCount=0` 保持无 B 帧,输入输出 1:1 配对不变,零延迟代价);High 档待实机验证驱动支持后可选;
  2. `ICodecAPI::SetValue`: `CODECAPI_AVEncCommonRealTime=TRUE`、`CODECAPI_AVEncCommonRateControlMode`(CBR 或 LowDelayVBR)、`CODECAPI_AVEncCommonMeanBitRate`(与 MF_MT_AVG_BITRATE 双写);
  3. **IDR 请求走 `CODECAPI_AVEncVideoForceKeyFrame`** 替代 `MFT_MESSAGE_COMMAND_FLUSH`——不丢帧、不清 rate-control 状态、无 1 帧空洞;flush 保留为不支持时的回退;
  4. 评估 `CODECAPI_AVEncSliceControlMode/Size`(按驱动支持):大 IDR(几百 KB)切片化,削掉单帧传输突发对端到端延迟的尖峰。
- 验证: 实机对比 Baseline vs Main 在同码率下的 SSD/主观画质与延迟;确认硬件 MFT 支持 ICodecAPI(多数 NVENC/QSV/AMF 的 MF 封装支持)。

### P1-2 码率自适应(拥塞不堆延迟,静屏不浪费)
- 数据源(现成/半现成): renderer `ping/pong` RTT(`nativeMediaSession.ts:188-190` + relay `:263-273`)、WS `bufferedAmount`、renderer 的解码丢弃计数。
- 策略: 每 500ms 采样;RTT 或丢弃率升高 → 经信令 `{t:"set-bitrate",bps}` → host → helper → napi `rd_media_set_bitrate`(新增,`ICodecAPI` 动态改 MeanBitRate,失败则按 P2 降级重建编码器);恢复时缓升。
- 交付建议: 自适应 v1 只做「两档」(8Mbps ↔ 4Mbps)+ 滞回,避免抖动。

### P1-3 画质档位与 GPU 能力探测(借鉴 UU远程 的能力缓存思路)
- 位置: 启动参数 `remoteDesktopServer.ts:558-566`(现为硬编码 1920/60/8Mbps)
- 改法: 暴露「清晰 / 均衡 / 流畅」档位(bitrate 8/5/3, fps 60/60/30,maxWidth 1920/1920/1600),由主控选择;被控端首次启动把 MF 编码器枚举结果 + 最大分辨率写入设置缓存(GPU 型号指纹做失效键),避免每会话重探测——即 UU远程 `encoder_codec_capability_cache.json` 的做法。

## P2 — 稳定性

### P2-1 断线与恢复
- 现状: renderer `ws.onclose → teardown → onTransportClosed` 单发事件,**无自动重连**(`nativeMediaSession.ts:161-175`);媒体票据 30s 单次消费,断开即不可续。
- 改法(分层):
  1. **媒体 WS 瞬断**: 信令 WS 尚存时,加 `{t:"request-media-ticket"}`(信令通道)→ host 重签 mediaTicket(re-derive 绑定同 clientId)→ renderer 重连 → 继续原密钥/盐流(helper/relay 未动,编码不停);
  2. **整会话断**: 客户端用退避(250ms→2s 封顶)重走 `POST /api/remote-desktop/session`;被控侧同意窗口适当延长(TTL 内重连不再弹同意);重连后首帧由 `force_idr` 保证 IDR;
  3. renderer 收到 `unfreeze`/重连续流后清 decoder 状态(close+configure)。
- 验收: 拔插网线/切 WiFi 恢复时间 <3s 且画面无花屏残留。

### P2-2 采集/编码故障恢复
- 现状: `worker_loop`(`napi.rs:316-363`)对采集错误/编码错误**直接退出**并只发 frozen 事件;lock/unlock(ACCESS_LOST)已有重开循环(:302-313),这条没问题。
- 改法: 错误退出前尝试一次**设备链重建**(新 D3D 设备 + 新 Encoder + import_state + force_idr,复用 `reopen_target` 的结构);重建失败才退出,由 JS 层(watchdog)自动 `rd_media_start` 重启 worker。加 encode 看门狗: `push()` 连续 >1s 未产出 → 记录并按重建处理。

### P2-3 helper/relay 生命周期
- helper 崩溃: broker/services 侧确认有自动拉起(`packages/mode-cua` broker 已有 helper 启动语义,需验证崩溃后下次调用能重启);relay 对 helper socket close 已有清理(:321-327),补一条「helper 断开 → N 秒内重连则继续会话」或在 helper 重启后自动 `rd_media_start`(当前需等下一轮 JS 编排,检查 `remoteDesktopServer.ts` MediaStart 流程的幂等)。
- relay `Buffer.from(payload)`(`:105`)去掉: 直接 `ws.send(payload)`(payload 已是 Buffer 视图,ws 发送时会内部入队复制)。

### P2-4 统计与可观测(对齐 WebRTC 路 `sessionStats`)
- 帧头扩展(建议在 T13/T14 一并做,趁契约 v2 未冻结): kind0 payload 增加 `[u32 capture_ts_ms]`(采集时刻,mod 2^32) → renderer 可计算**帧龄(glass-to-glass 近似)**,不用改加密布局(放在 flags 后、密文前,或纳入 flags 后的明文小头)。
- 面板指标: 帧龄 P50/P95、RTT、解码丢弃数、解码队列长度、码率实际值、fps、freeze 次数。没有这些,后续任何优化都无法验收。

### P2-5 压力与回归 harness(扩展 .spike/remote-desktop)
- 输入→显示延迟: 被控端全屏闪块(100ms 周期黑白)+ 主控端高帧率摄像头/录屏点测,或高精度时钟对拍(现有 `main-webrtc-pair.mjs` 的时钟法可复用);
- 场景矩阵: 静态桌面光标随动(验 P0-2 修复)、4K→1080 缩放、锁屏/解锁、UAC 弹窗、显示器热插拔/换屏(现有 switch 路径)、GPU 切换(双显卡笔记本)、WiFi 丢包注入(`clumsy`/`netem` 等价物)、2h soak(内存与句柄曲线)。

## 与在途任务(T10–T15)的整合建议

- **T13(renderer 切换)**: 吸收 P0-4(importKey/门控/desynchronized/seq 防乱序)、P0-5 的 renderer 侧 `{t:"need-key"}`、P2-1 的 decoder 重置语义。
- **T14(媒体输入接线)**: 吸收 P0-5 控制链(relay → helper → `rd_media_request_key`)、P0-6 中 helper/relay 的 noDelay、P1-2 的 `rd_media_set_bitrate` 预留;输入上行保持 event-driven(不要复用 WebRTC 路的 main 轮询模式)。
- **新增独立任务(T16+)**: P0-1(GPU 互操作)、P0-2(GPU 光标)、P1-1(CODECAPI)、P2-1(重连)、P2-4(帧龄统计)。P0-1/P0-2 在 Rust 侧,与 JS 侧任务可并行。

## 风险与回退
- P0-1/P0-2 涉及驱动行为差异: 两项都设显式开关 + 保留现路径;`rd_media` 已有纯逻辑测试面(tests/rd_media.rs),新增路径先落状态机/纯函数测试,再进实机。
- P0-5 的 IDR 请求依赖 `force_idr` 语义(现已存在且被 switch/解冻使用),风险低;注意 flush 回退路径仍丢 1 帧,属已接受行为。
- P1-1 Main profile 若某驱动不支持 → 回退 Baseline(一行),不阻塞其它项。
- 所有「预期收益」为链路估算,最终以 P2-5 harness 的实测帧龄为准。

## 附: 本轮不动的项(明示)
- 混合 DPI/多显示器坐标(一期已知限制,与延迟优化解耦);
- `D3D11_VIDEO_USAGE_PLAYBACK_NORMAL` → `OPTIMAL_SPEED`: 属画质/延迟权衡项,建议实机 A/B 后再定;
- 异步 MFT(METransformNeedInput 事件驱动): 收益小于 P0-1,复杂度高,暂缓;
- UDP/QUIC 传输、剪贴板、音频: 按 spec 既定非目标。

---

## 附二:与在途迁移计划(native-media T1–T15)的对比(2026-10-12 深夜核实)

在途优化 = `docs/superpowers/plans/2026-10-12-lan-remote-desktop-native-media.md`(WebRTC → 原生直替,目标 25–40ms)。
核实时点真实进度: **T1–T9 已提交**(`git log` 至 `cafd592`,16:54–23:52);**T10(renderer 切换)进行中**(`RemoteDesktopSession.tsx:208` 仍 `<video>`、`useRemoteDesktopSession.ts:176` 仍 `RTCPeerConnection`);T11–T15 待做;两台实机验收待用户执行。
注意: 计划文件中「实现进度 P1–P4 [x]」一段是 **T14 Step 3 待写入 spec 的模板文本**,不是现状。

对比结论(我的 P0–P2 与迁移计划的关系):

| 本方案项 | 迁移计划覆盖 | 处置 |
|---|---|---|
| P0-1 GPU 互操作(去 NV12 回读) | ❌ 未覆盖——实现按 spec「降级预案」走系统内存 | 迁移后新任务(恢复 spec 主案;Rust 侧,可与 JS 并行) |
| P0-2 光标 GPU 合成 + 指针帧修复 | ❌ CPU 变体为 spec 降级预案;指针-only 帧丢弃是既有缺陷 | 同上;指针帧问题需实机先确认 |
| P0-3 采集帧直通 VP | ❌ 依赖 P0-2 | 随 P0-2 |
| P0-4 renderer 热路径(importKey/门控/desync/防乱序) | ⚠️ T9 已提交代码存在;T10 重写 hook 但计划文本未含 | **折进 T10 窗口**(模块刚落地,改动最小) |
| P0-5 背压阈值 + need-key IDR 通道 | ❌ 4MB 丢弃是 T5 既定行为;无 IDR 请求路径 | **折进 T14**(输入接线同一 WS 控制面;relay/helper/napi 各一处) |
| P0-6 TCP_NODELAY | ❌ 全仓库零使用 | 小修复,随 T10–T14 任一提交 |
| P1-1 Main profile + CODECAPI(ForceKeyFrame 等) | ⚠️ 现状 Baseline + flush-IDR 是 spec Resolution 2 的既定取舍 | 迁移后 Rust 侧新任务(是对 R2 的改进:不丢帧、不清 RC) |
| P1-2 码率自适应 | ❌ 固定 8Mbps | 迁移后;复用 T11 的 RTT + T14 的 WS 控制面 |
| P1-3 画质档位 + 能力缓存 | ❌ | 迁移后 |
| P2-1 断线重连/会话续用 | ❌ teardown 单发;T11 只加 RTT 读数 | 迁移后(媒体票据重签可借 T13 信令 v2 面) |
| P2-2 采集/编码故障重建 | ❌ worker 出错即退出 | 迁移后 Rust 侧 |
| P2-3 helper 生命周期 / relay 去复制 | ❌ | 小修复,随时 |
| P2-4 帧头 capture_ts(帧龄统计) | ⚠️ T11 只有 fps/RTT/分辨率 | **趁 T10 与 T12/T13 契约冻结前一并做**(帧头/消费方都在改) |
| P2-5 实测 harness | ⚠️ T15 回归 + spike 已有单机环回 | 并入 T15 验收准备 |

要点: 迁移计划为"直替 WebRTC、先跑通"选了 spec 的降级实现(系统内存回读 / CPU 光标 / Baseline / flush-IDR / 4MB 丢弃);本方案的 P0 项不是另一条路线,而是**把 spec 主案补完 + 补齐两个闭环缺失(丢帧→IDR、指针帧)**,使 25–40ms 预算在迁移合并后可达。

## 附三:执行分工与进度(2026-10-13 半夜)

**分工原则(两边并行,写者分离):**
- 迁移会话:继续 T10–T15,只碰 `packages/**`、`docs/specs/**`。
- 本方案执行:Rust 面(`crates/mode-cua-ax/**`),独立 worktree `.worktrees/rd-media-perf`(分支 `feat/rd-media-perf`,从 cafd592 拉出)。两边文件零重叠;待迁移 T15 落地后合并(预期无冲突)。
- 不并行改的:JS 小项(importKey/NoDelay/背压阈值/need-key 的 JS 侧)由迁移会话在其 T10/T14 窗口顺手做;帧龄字段趁 T12/T13 契约冻结前定。

**已完成(feat/rd-media-perf):**
- `cd13a57` perf(cua): 编码输入走 D3D11↔MF 互操作(MFT_MESSAGE_SET_D3D_MANAGER + MFCreateDXGISurfaceBuffer;建管/类型协商/运行中三层回退到系统内存;NV12 纹理补 SRV 位)——即 P0-1。
- `994ca66` feat(cua): `rd_media_request_key` napi 导出 + worker 一次性消费(即 P0-5 的 Rust 侧,JS 侧留 T14)。
- 门禁:`cargo check` / `cargo test`(11/11) / `cargo clippy -D warnings` 全绿(gnu + llvm-mingw 路线)。

**下一步(Rust 面):** P0-2 光标 GPU 合成 + `capture.rs:341` 纯指针帧处理(需实机确认后定改法);P1-1 CODECAPI/Main profile。

## 附四:执行进度更新(2026-10-13 凌晨)

**feat/rd-media-perf 现有 5 个提交(全部门禁绿:check / cargo test 12/12 / clippy -D warnings):**
- `cd13a57` perf(cua): D3D11↔MF 互操作编码输入(三层回退) — P0-1
- `994ca66` feat(cua): `rd_media_request_key` 紧急关键帧控制面 — P0-5 Rust 侧
- `f994a49` fix(cua): 光标合成重构 — `clean` 层 + 指针-only 重合成(静置画面光标随动)+ 形状 `0=未变` 语义修正 + 栅格随形状一次渲染 — P0-2a(含两个功能缺陷修复)
- `35efe2b` perf(cua): Main profile 能力阶梯 + ICodecAPI 低延迟参数(RealTime/B帧=0)+ ForceKeyFrame 免丢帧 IDR(输出侧验证,未生效自动降级 flush) — P1-1
- `5c817e9` feat(cua): `rd_media_set_bitrate` 动态码率控制面(ICodecAPI MeanBitRate) — P1-2 Rust 侧

**主线(native-media 迁移)进度:** T10/T11/T12/T13 已提交(至 ec0a8da 信令 v2 + 冻结 UI),`crates/` 零改动 → 与本分支零交集,待 T15 后合并预期无冲突。

**Rust 面剩余(非阻塞):**
- P0-2b 光标 GPU 合成(替代 CPU 回读)——**需要实机验证驱动行为后实施**;P0-3(采集帧直通 VP)依赖它。
- P2-2 采集/编码错误的一次性自愈重建(worker 层,现为出错即退出)。
- 帧头 capture_ts(帧龄统计)待 JS 侧 T12/T13 契约决策;若采纳,由 Rust 侧补字段。

**验收清单增补(实机执行):**
- 对端画面静止时拖动鼠标 → 远端光标应平滑随动(验证 f994a49)。
- 常态录屏对比:同码率下画质优于旧版(验证 Main profile 生效);若日志出现「回退 Baseline」说明驱动仅支持 Base。
- 断网/丢帧恢复:画面应在 ≤1 个 RTT + 编码延迟内重新出关键帧(验证 ForceKeyFrame/need-key 链路)。

## 附五:合并完成(2026-10-13 凌晨)

- 迁移会话 T14/T15 落地(WebRTC 面全删、onInput 接线、验收交付),`crates/` 全程零改动。
- `feat/rd-media-perf` rebase 到主线后 **fast-forward 合并**(线性历史,零冲突)。主线现有 5 个 Rust 提交:
  `0a00f003` D3D11↔MF 互操作 · `686cfd97` request_key · `7e947cdc` 光标合成重构 · `7941ce13` 编码档位/ForceKeyFrame · `6d4a70fd` set_bitrate。
- 合并后主线门禁:cargo test(8 套件 **67 个**全过)+ clippy -D warnings 绿。
- worktree 与分支已清理;`pnpm build:cua-helper` 重建本地 helper 产物(新版 `rd_media_request_key`/`rd_media_set_bitrate` 符号落盘)。
- 追加:`657761d1` **P2-2 错误自愈**——采集错误 → 重建采集链(新设备+新编码器,状态机迁移);编码错误 → 重建编码器;均失败才上报 frozen 退出。

**剩余(非阻塞):** P0-2b 光标 GPU 合成(待实机验证驱动);帧头 capture_ts(待 JS 侧契约);两台实机验收(用户执行,清单见「附四」)。

## 附六:收官批追加四项(2026-10-13 凌晨,"能做全部做完")

- `06deb575` **传输闭环**:中继背压 4MB→**128KB**且丢帧爆发**立即请关键帧**(kind3)、`{t:"need-key"}`/`{t:"set-bitrate"}` 上行控制面、helper/relay/媒体+信令 WS 全链路 **NoDelay**、中继去 `Buffer.from` 拷贝。
- `74bcb88c` **renderer 热路径**:AES key 会话级导入(原每帧 importKey)、seq 防乱序水位、解码队列 >2 丢 delta + need-key、canvas `desynchronized`。
- `(Batch3)` **自适应码率 v1**:`adaptiveBitrate.ts` 纯逻辑(RTT+丢弃采样,两档 8↔4Mbps,非对称滞回:降快升慢),hook 在 pong 节拍驱动,经 set-bitrate 下行到 `ICodecAPI`。
- `(Batch4)` **端到端帧龄**:Rust 取帧打点(`captured_at_ms`,u32 Unix 毫秒)→ napi → **契约 B v2 帧头 [seq][flags][ts][ct]** → renderer 解析;relay pong 附 `hostTs` 供时钟偏移标定(offset ≈ hostTs−ts−RTT/2);状态条新增「端到端 ~N ms」(窗口中位数),三语同步。

门禁:cargo 67 测试 + clippy 绿;helper 133、desktop 17、UI 119 全过;typecheck/lint/语言政策绿;`pnpm build:cua-helper` 产物已含新字段。

## 附七:远程控制交互五项改造(2026-10-13)

1. **同意窗无法关闭(修复)** `eb88f40a`:coordinator 在 open-consent 里提前把 consentWindow 置空,导致 transition 的 close-consent 动作拿不到引用、弹窗永不 destroy。改由动作统一 close+置空(consent-decided 动作集必含 close-consent;timeout/收口路径本就二次 close,幂等)。
2. **向导直选已配对设备** `5ea4c8f5`:局域网连接向导新增「已配对设备」列表(getLanPairedPeers),点选即填地址与 serverId(令牌由 Host 按 serverId 取,免配对码);手工改动地址自动脱离直选上下文。
3. **设备页重命名/删除** `84c6cabc`:新增 `renameLanPairedPeer(serverId,name)` 全平台链(契约 channels/validation → main → host 元数据写 → preload → Web 桩);设备卡三操作:连接/重命名(inline,Enter 保存/Esc 取消)/删除(两段式确认)。
4. **检测设备+配对前移到远程控制页** `b6a95671`:新增 `RemoteDesktopAddDevice` 组件(扫描 discoverLanPeers + 选中设备输配对码 pairLanPeer,成功即刷新设备列表);远程连接向导原有流程保留(新建局域网工作区仍走它)。
5. **合并为「远程控制」** `4156f006`:侧边栏入口/页面标题/面包屑/设置分区标题统一为「远程控制」(三语);设置页「远程桌面」与「局域网访问」两分区合并为一个 remoteDesktop 分区(被控设置 + 本机共享同区渲染),`lanAccess` 保留为历史意图别名。
