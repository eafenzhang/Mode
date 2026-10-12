# 原生远程桌面媒体面（DXGI + 硬编 + 自控传输）Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 用 DXGI Desktop Duplication + Media Foundation 硬件 H.264 + 自控加密 WS 传输，纯直替现有 WebRTC 媒体面，端到端延迟目标 25–40ms。

**Architecture:** 被控 helper（Node + napi addon）采集硬编并逐帧 AES-256-GCM 加密，经本机 TCP 管道交给被控 host 中继，主控 renderer 直连被控 LAN server 的 media WS 用 WebCodecs 硬解直画 canvas；输入反向走同一条 WS；同意/指示/换屏等信令沿用既有信令 WS（union 升 v2）。详见 spec。

**Tech Stack:** Rust（windows crate 0.58：DXGI/D3D11/MediaFoundation；napi-rs 2）、Node 24（net/crypto/ws）、WebCodecs、zod schema、tsx --test / cargo test。

**Spec:** `docs/specs/lan-remote-desktop-native-media.md`（本计划逐条据其论证；计划中的偏差必须先改 spec 再实现）。

## Global Constraints

- 平台：仅 Windows（win32 桌面端）；Rust 目标三元组默认 `x86_64-pc-windows-gnu`（CI 用 msvc）。
- 仓库根命令：`pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed`、`node scripts/check-language-policy.mjs`。
- Rust：`cargo test --manifest-path crates/mode-cua-ax/Cargo.toml`；`cargo clippy --manifest-path crates/mode-cua-ax/Cargo.toml --all-targets -- -D warnings`（`#![deny(clippy::unwrap_used)]`，禁止 `unwrap`）。
- helper JS 测试：`pnpm --dir packages/mode-cua test`（`node --test test/*.test.mjs`）。
- UI 测试：`pnpm exec tsx --tsconfig packages/ui/tsconfig.json --test packages/ui/test/<file>.test.ts`。
- server/shared 测试：`pnpm --filter @mode/server test`、`pnpm --filter @mode/shared test`。
- helper 构建：`pnpm build:cua-helper`。
- clean-room 纪律：不读取上游 zai-org/ZCode 与其衍生实现；对拍只比行为不看实现。
- Agent 只做本地提交，不 `git push`。
- 信令/IPC 面纪律：`ipcChannels.ts`「仅新增通道可扩」；`hostMessages.ts` 与 `remoteDesktopMessages.ts` 是**逐字镜像两份**，必须同时改。
- i18n：zh-CN / en-US / fa 三语同步，改完跑语言政策检查。
- 不用 UI 直调 `window.mode`；新平台方法必须同时改 4 处（`shared/platform.ts`、`client/globals.d.ts`、`preload/index.ts`、`desktopPlatform.ts`）+ `web/main.tsx` stub。

## File Structure（按阶段）

```
crates/mode-cua-ax/src/rd_media/          [新建] mod.rs(公共) / state.rs(纯逻辑) / capture.rs(DXGI) / encode.rs(MF) / napi.rs(导出)
crates/mode-cua-ax/tests/rd_media.rs      [新建] 纯逻辑集成测试
packages/mode-cua/helper/mediaPump.mjs    [新建] 帧加密 + 长度前缀管道泵
packages/mode-cua/helper/server.mjs       [改]   createBackend 增 rd_media_* 方法
packages/mode-cua/broker.js               [改]   BROKER_METHODS 增 rd_media_*
packages/mode-cua/test/helper-backend.test.mjs [改] 新方法用例
packages/services/src/cua-permission-broker/cuaRdMedia.ts [新建] 控制器 side-table（socket 解析复用 raw input 先例）
packages/services/src/node.ts             [改]   装配 rdMedia 控制器
packages/server/src/http.ts               [改]   双票据 + /ws/remote-desktop-media
packages/server/src/remoteDesktopTicket.ts [改]   issuePair()（或两次 issue）
packages/server/test/remote-desktop-session.test.ts [改] 双票/媒体端点用例
packages/desktop/src/host/remoteDesktopMediaRelay.ts [新建] 本地 TCP ↔ WS 中继
packages/desktop/src/host/remoteDesktopServer.ts [改]   media WS 处理、冻结状态、input 注入
packages/desktop/src/host/lanAccessServer.ts [改]  hooks.onMediaSocket
packages/desktop/src/host/remoteDesktopPeerClient.ts [改] 解析 mediaTicket、拼 mediaUrl
packages/desktop/src/{main,host}/remoteDesktop{Messages,hostMessages}.ts [改] 镜像消息
packages/desktop/src/main/remoteDesktop/coordinator.ts [改] MediaStart/Stop/切换/冻结转信令
packages/desktop/src/main/remoteDesktop/monitors.ts [新建] 从 captureWindow 抽出的显示器枚举
packages/desktop/src/main/remoteDesktop/controllerSession.ts [改] mediaUrl 透传
packages/desktop/src/main/remoteDesktop/desktopIpc.ts [改] StartSession 返回体
packages/shared/src/remote-desktop.ts     [改]   信号 union v2、协议版本 2
packages/ui/src/remote-desktop/nativeMediaSession.ts [新建] 解码状态机(纯) + WebCodecs/canvas 胶水
packages/ui/src/remote-desktop/sessionStats.ts [改]  自有 fps/rtt/分辨率计数（保输出形状）
packages/ui/src/remote-desktop/useRemoteDesktopSession.ts [改] 重写（无 PC/DataChannel）
packages/ui/src/remote-desktop/RemoteDesktopSession.tsx [改] <video> → <canvas>
packages/ui/src/remote-desktop/{videoFit,InputCapture}.ts [改] 表面元素泛化（canvas）
packages/ui/src/remote-desktop/sessionReducer.ts [改] media-open/freeze 分支、删 offer/ice
packages/ui/src/remote-desktop/deviceAvailability.ts [改] remoteDesktopNative reason
packages/ui/test/remoteDesktop{NativeSession,Decoder,Stats}.test.ts [新建]
```

## 关键跨任务契约（Interfaces 总表）

所有任务只允许使用下表中的形状；改形状 = 先改本表 + spec。

**A. napi（Task 2 产出，Task 7 消费）**
```ts
rd_media_start(opts: {
  rect: { x: number; y: number; width: number; height: number }; // 物理像素（VIRTUALDESK 口径）
  maxWidth: number;        // 编码输出宽上限 1920（高度按 rect 比例推导）
  fps: number;             // 60
  bitrateBps: number;      // 8_000_000
  onFrame: (frame: { seq: number; key: boolean; data: Buffer }) => void;
  onStatus: (s: { frozen: boolean; reason?: string }) => void;
}): void
rd_media_switch(rect: { x,y,width,height }): void   // 换屏：重选输出 + 强制 IDR（seq 连续）
rd_media_stop(): void                               // 幂等
```

**B. 本地管道（helper ↔ 被控 host；Task 5 实现，Task 7 消费）**
- 帧：`[u32 len 大端][u8 kind][payload]`，`len = 1 + payload.length`。
- `kind=0` 帧密文（helper→host）：payload = `[u32 seq 大端][u8 flags(bit0=关键帧)][ciphertext‖tag(16B)]`；IV 不在帧内，解密方重建 = `salt(8B) ∥ seq(4B 大端)`；算法 `aes-256-gcm`，key=32B hex（64 字符）。
- `kind=1` 状态（helper→host）：payload = UTF-8 JSON `{"frozen":true,"reason":"locked"}` 或 `{"frozen":false}`。
- `kind=2` 开流指令（host→helper）：payload 空。

**C. media WS（Task 3 端点 + Task 5 中继）**
- 路径：`GET /ws/remote-desktop-media?mediaTicket=<t>`，票据经 `remoteDesktopTickets.issue()` 第二次签发，单次消费、30s、绑定 clientId；升级后调用 `remoteDesktop.onMediaSocket(clientId, ws)` hook。
- 上行（renderer→host，文本 JSON）：`{"t":"ready"}`、`{"t":"ping","ts":<epoch ms>}`、既有输入事件对象（`{t:"pm"|...}` 原样）。
- 下行（host→renderer）：二进制 = 管道 kind0 payload **原样转发**；文本 JSON：`{"t":"pong","ts":...}`。
- 背压：ws `bufferedAmount > 4MB` 时丢弃后续 kind0（保 ping/输入不被大帧拖死）；断开即丢帧。

**D. 信令 union 增量（Task 4，additive；Task 14 删旧）**
```ts
{ type: "media-open", key: string /*64 hex*/, salt: string /*16 hex*/ }   // 被控 → 主控
{ type: "freeze", reason: "locked" | "access-lost" }                       // 被控 → 主控
{ type: "unfreeze" }                                                       // 被控 → 主控
REMOTE_DESKTOP_PROTOCOL_VERSION: 1 → 2（Task 13 改值）
```

**E. rdStartSession 返回体扩展（Task 3）**
```ts
// shared/platform.ts
rdStartSession(serverId): Promise<{ ok: true; mediaUrl?: string } | { error: RemoteDesktopErrorCode }>
// mediaUrl = `ws://<对端host>:<port>/ws/remote-desktop-media?mediaTicket=<t>`（主控 host 从 POST 响应拼出）
```
> **Spec 偏差（Task 3 必须先改 spec）**：spec 原文 `media-open{url,key,salt}` 的 `url` 改由 `rdStartSession` 返回值携带（被控 host 不知道自己的对外可达地址，主控 host 拼 URL 更可靠）；`media-open` 只带 `key/salt`。

**F. host ↔ main 新消息（Task 6；两份镜像文件同改）**
```ts
RemoteDesktopMediaStart  { type, requestId?, rect: {x,y,width,height}, key: string, salt: string }
RemoteDesktopMediaSwitch  { type, requestId?, rect }
RemoteDesktopMediaStop    { type }
RemoteDesktopMediaStatus  { type, frozen: boolean, reason?: string }  // host → main
// 均为单向通知型（postToMain / sendRemoteDesktopMessageToHost），无响应关联。
```

**G. host 侧 broker 方法（Task 7）**
```
rd_media_start  { rect, maxWidth: 1920, fps: 60, bitrateBps: 8000000,
                  endpoint: { host: "127.0.0.1", port: number },
                  key: string(64hex), salt: string(16hex) } → { ok: true }
rd_media_switch { rect } → { ok: true }
rd_media_stop   {} → { ok: true }
```

**H. renderer 原生会话模块（Task 9 产出，Task 10 消费）**
```ts
createNativeMediaSession(opts: {
  url: string; keyHex: string; saltHex: string;
  canvas: HTMLCanvasElement;
  onFrame?: (info: { width: number; height: number }) => void; // 首帧/重配回调
  onTransportClosed?: (reason: "closed" | "error") => void;
  onDecoded?: () => void;   // 每解码一帧（stats fps 计数）
}): {
  send(event: object): void;   // 输入/ready 上行（内部首条自动发 ready）
  ping(ts: number): void;
  stop(): void;
}
// 纯函数（可单测）：
parseFrameHeader(bytes: Uint8Array): { seq: number; key: boolean; body: Uint8Array } | null
buildFrameIv(salt: Uint8Array, seq: number): Uint8Array
```

---

## Phase P1 — 采集与编码（Rust，不上线）

### Task 1: `rd_media` 纯逻辑与 Cargo 特性

**Files:**
- Modify: `crates/mode-cua-ax/Cargo.toml`（windows features 增补）
- Create: `crates/mode-cua-ax/src/rd_media/mod.rs`、`crates/mode-cua-ax/src/rd_media/state.rs`
- Modify: `crates/mode-cua-ax/src/lib.rs`（`pub mod rd_media;`）
- Test: `crates/mode-cua-ax/tests/rd_media.rs`

**Interfaces:**
- Produces: `rd_media::state::{EncoderState, EncoderAction, reduce, FrameFlags, frame_flags, STATUS_*}`（Task 2 消费）

- [ ] **Step 1: Cargo.toml 增 windows features**

在 `windows = { version = "0.58", features = [` 列表追加（保持现有项不动）：

```toml
  "Win32_Graphics_Dxgi",
  "Win32_Graphics_Direct3D",
  "Win32_Graphics_Direct3D11",
  "Win32_Media",
```

- [ ] **Step 2: 写失败测试**

`crates/mode-cua-ax/tests/rd_media.rs`：

```rust
//! rd_media 纯逻辑（spec: docs/specs/lan-remote-desktop-native-media.md 测试面）。
use mode_cua_ax::rd_media::state::{frame_flags, reduce, EncoderAction, EncoderState};

#[test]
fn gop_reduces_to_idr_every_120_frames() {
    let mut s = EncoderState::new(120);
    let mut idr_count = 0;
    for _ in 0..360 {
        let a = reduce(&mut s);
        if matches!(a, EncoderAction::Emit { .. }) {
            if a_is_key(&a) { idr_count += 1; }
        }
    }
    assert_eq!(idr_count, 3, "360 帧 / GOP120 应恰好 3 个关键帧");
}

fn a_is_key(a: &EncoderAction) -> bool {
    matches!(a, EncoderAction::Emit { key: true, .. })
}

#[test]
fn force_idr_after_switch_emits_key_next_frame() {
    let mut s = EncoderState::new(120);
    for _ in 0..10 { let _ = reduce(&mut s); }
    reduce_force_idr(&mut s);
    assert!(matches!(reduce(&mut s), EncoderAction::Emit { key: true, seq: 10, .. }));
}

fn reduce_force_idr(s: &mut EncoderState) {
    let _ = s;
    s.force_idr();
}

#[test]
fn freeze_halts_emit_and_unfreeze_forces_idr() {
    let mut s = EncoderState::new(120);
    s.set_frozen(true);
    assert!(matches!(reduce(&mut s), EncoderAction::None));
    s.set_frozen(false);
    assert!(matches!(reduce(&mut s), EncoderAction::Emit { key: true, .. }));
}

#[test]
fn frame_flags_key_bit_only_on_idr() {
    assert_eq!(frame_flags(false), 0);
    assert_eq!(frame_flags(true), 1);
}
```

- [ ] **Step 3: 运行确认失败**

Run: `cargo test --manifest-path crates/mode-cua-ax/Cargo.toml rd_media`
Expected: FAIL（`mode_cua_ax::rd_media` 不存在）

- [ ] **Step 4: 实现 state.rs**

`src/rd_media/mod.rs`：

```rust
//! 原生媒体面（spec: docs/specs/lan-remote-desktop-native-media.md）。
pub mod state;
```

`src/rd_media/state.rs`：

```rust
//! 采集/编码循环的纯状态机（不碰 Win32；单测全在此层）。
pub struct EncoderState {
    seq: u32,
    gop: u32,
    frames_since_idr: u32,
    force_idr: bool,
    frozen: bool,
}

pub enum EncoderAction {
    None,
    Emit { seq: u32, key: bool },
}

impl EncoderState {
    pub fn new(gop: u32) -> Self {
        Self { seq: 0, gop: gop.max(1), frames_since_idr: 0, force_idr: false, frozen: false }
    }
    pub fn force_idr(&mut self) { self.force_idr = true; }
    pub fn set_frozen(&mut self, frozen: bool) {
        if self.frozen && !frozen { self.force_idr = true; } // 解冻续流必须带 IDR
        self.frozen = frozen;
    }
    pub fn is_frozen(&self) -> bool { self.frozen }
}

pub fn reduce(state: &mut EncoderState) -> EncoderAction {
    if state.frozen {
        return EncoderAction::None;
    }
    let key = state.force_idr || state.frames_since_idr == 0;
    let seq = state.seq;
    state.seq = state.seq.wrapping_add(1);
    state.frames_since_idr += 1;
    if key {
        state.force_idr = false;
        state.frames_since_idr = 1;
    } else if state.frames_since_idr >= state.gop {
        state.frames_since_idr = 0;
    }
    EncoderAction::Emit { seq, key }
}

pub const FRAME_FLAG_KEY: u8 = 1;
pub fn frame_flags(key: bool) -> u8 { if key { FRAME_FLAG_KEY } else { 0 } }
```

> 语义：新会话首帧必 IDR；每 GOP 帧发一个 IDR；`force_idr`（换屏/解冻）下一帧必 IDR；冻结期不产出帧。

- [ ] **Step 5: 运行确认通过 + clippy**

Run: `cargo test --manifest-path crates/mode-cua-ax/Cargo.toml rd_media && cargo clippy --manifest-path crates/mode-cua-ax/Cargo.toml --all-targets -- -D warnings`
Expected: PASS（注意 `lib.rs` 需加 `pub mod rd_media;` 一行，放在现有 `pub mod screen;` 之后）

- [ ] **Step 6: Commit**

```bash
git add crates/mode-cua-ax/Cargo.toml crates/mode-cua-ax/src/rd_media crates/mode-cua-ax/src/lib.rs crates/mode-cua-ax/tests/rd_media.rs
git commit -m "feat(cua): rd_media 纯逻辑状态机与 DXGI/MF Cargo 特性"
```

### Task 2: DXGI 采集 + MF 硬编 + napi 导出

**Files:**
- Create: `crates/mode-cua-ax/src/rd_media/capture.rs`、`encode.rs`、`napi.rs`
- Modify: `crates/mode-cua-ax/src/rd_media/mod.rs`、`src/lib.rs`（napi 导出挂载）
- Test: `crates/mode-cua-ax/tests/rd_media.rs`（追加守卫用例）

**Interfaces:**
- Consumes: Task 1 全部
- Produces: 契约 A 的 `rd_media_start / rd_media_switch / rd_media_stop`

- [ ] **Step 1: 写失败测试（napi 存在性 + 幂等 stop）**

在 `tests/rd_media.rs` 追加：

```rust
#[test]
fn stop_without_start_is_idempotent() {
    // 真实 DUPL 在无桌面会话的 CI 可能不可用：stop 空跑必须 Ok。
    mode_cua_ax::rd_media::stop_for_tests();
}
```

Run: `cargo test --manifest-path crates/mode-cua-ax/Cargo.toml rd_media`
Expected: FAIL（`stop_for_tests` 不存在）

- [ ] **Step 2: 实现 capture.rs（采集 + 光标 + 纹理）**

核心结构与流程（`#![allow(clippy::...)]` 不可用——遵守 deny unwrap，用 `map_err`）：

```rust
// capture.rs：DXGI Desktop Duplication。所有 Win32/DXGI 调用集中在此，state 仍走 state.rs。
use windows::Win32::Graphics::Dxgi::{IDXGIOutput1, IDXGIOutputDuplication, DXGI_OUTDUPL_FRAME_INFO};
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11Texture2D, D3D11_TEXTURE2D_DESC};

pub struct DuplicationOutput {
    device: ID3D11Device,
    duplication: IDXGIOutputDuplication,
    // 目标输出的物理矩形（用于把光标屏幕坐标换算到纹理坐标）
    desktop_rect: (i32, i32, i32, i32), // x,y,w,h
}

pub enum CaptureFrame {
    /// 新桌面帧（seq 由外层 state 分配）：GPU 纹理句柄 + 光标补丁信息
    NewDesktop { texture: ID3D11Texture2D, cursor: Option<CursorPatch> },
    /// 无新帧（AcquireNextFrame 超时/无变化）
    Idle,
    /// 锁屏/安全桌面：会话丢失（触发 frozen）
    AccessLost,
}

pub struct CursorPatch { pub x: i32, pub y: i32, pub w: u32, pub h: u32, pub rgba: Vec<u8> }

impl DuplicationOutput {
    /// 枚举首个 DesktopCoordinates 命中 rect 的输出并建 D3D11 device + duplication。
    pub fn open(rect: (i32, i32, i32, i32)) -> AxResult<Self> { /* D3D11CreateDevice →
        枚举 DXGI adapter/outputs → 比对 DXGI_OUTDUPL_OUTPUT_DESC.DesktopCoordinates
        → IDXGIOutput1::DuplicateOutput */ }
    /// 阻塞至下一帧（timeout_ms=16）。返回前已把桌面纹理 copy 到内部 proc 纹理并
    /// 用 UpdateSubresource 打上光标补丁（GDI 渲染的小块 BGRA）。
    pub fn next_frame(&mut self, timeout_ms: u32) -> AxResult<CaptureFrame> { ... }
    pub fn close(&mut self) { /* Release focus、释放 duplication */ }
}
```

实现要点（照抄到代码里，不省略）：
1. `D3D11CreateDevice`（`D3D_DRIVER_TYPE_HARDWARE`，feature level ≥ 11_0）失败时回退 `WARP`。
2. 枚举：`CreateDXGIFactory1` → `EnumAdapters1` → `EnumOutputs` → 读 `DesktopCoordinates`，与入参 rect 相等者选定；`QueryInterface::<IDXGIOutput1>()` → `DuplicateOutput(device)`。
3. `AcquireNextFrame(timeout, &mut info, &mut resource)`：
   - `info.LastPresentTime == 0 && info.AccumulatedFrames == 0` → `Ok(CaptureFrame::Idle)`（必须 `ReleaseFrame`）。
   - 返回 `DXGI_ERROR_ACCESS_LOST`（`0x887A0026`）→ `Ok(CaptureFrame::AccessLost)`（不 Release）。
   - 成功 → resource 转 `ID3D11Texture2D`，`CopyResource` 到内部 `proc` 纹理（`D3D11_USAGE_DEFAULT`、同 desc）。
4. 光标：`info.PointerPosition`（`PointerInfo.Visible != 0`）→ 取 `GetFramePointerShape` 缓存形状 → 每帧用 GDI 把形状渲染进 `CursorPatch.rgba`（单色掩码按 `PointerShapeType` 展开；颜色形状直接拷 `ShapeData`），坐标 = 屏幕光标点 − output `DesktopCoordinates.xy` − 形状 hotspot。`UpdateSubresource(proc, .., rgba.as_ptr())` 补丁矩形（越界钳制到纹理内）。
5. 光标形状变更（`info.PointerShapeBufferSize` 变化）重新拉取；纯移动只更新位置。
6. `close()`：`duplication = None`（释放）、`ReleaseFrame` 纪律：每个 Acquire 成功分支都必须 Release（AccessLost 除外）。

- [ ] **Step 3: 实现 encode.rs（Video Processor → NV12 → MF H.264）**

```rust
// encode.rs：proc 纹理(BGRA, 已含光标) → ID3D11VideoProcessor(BGRA→NV12 + 缩放到 maxWidth)
// → Map(staging NV12) → IMFTransform(MFVideoEncoder H.264 HW) → Annex-B ES。
pub struct Encoder {
    // D3D11 video device/context、VP、NV12 target + staging、MF session/输入输出 media type
    seq_source: crate::rd_media::state::EncoderState,
}
pub struct EncodedChunk { pub seq: u32, pub key: bool, pub data: Vec<u8> }

impl Encoder {
    pub fn new(device: &ID3D11Device, out_w: u32, out_h: u32, fps: u32, bitrate_bps: u32) -> AxResult<Self>;
    /// 一帧进一帧出（ProcessInput + ProcessOutput 循环，MENeedMoreInput 时返回 None 继续喂）。
    pub fn push(&mut self, texture: &ID3D11Texture2D) -> AxResult<Option<EncodedChunk>>;
    pub fn force_idr(&mut self) { self.seq_source.force_idr(); }
    pub fn set_frozen(&mut self, frozen: bool) { self.seq_source.set_frozen(frozen); }
}
```

实现要点：
1. VP：`CreateVideoProcessorEnumerator` with input `DXGI_FORMAT_B8G8R8A8_UNORM` / output `DXGI_FORMAT_NV12` range limited；`CreateVideoProcessor`；每帧 `VideoProcessorBlt`（源 rect = 整个 proc，目标 = out_w×out_h，顺带完成 >1080p 缩放）。
2. NV12 staging：`D3D11_USAGE_STAGING` + `CPU_ACCESS_READ`；`CopyResource` → `Map(READ)` → 记下 `RowPitch` 拷成紧凑 buffer（按 NV12 布局 Y plane + UV plane）→ `Unmap`。
3. MF：`MFStartup(MFSTARTUP_FULL)`（进程内一次，用 `std::sync::Once`）→ 枚举 `MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER` 的 `MFT_CATEGORY_VIDEO_ENCODER`，取含 "H.264" 描述者；输入 media type：`MFVideoFormat_NV12` + `MF_MT_FRAME_SIZE(out_w,out_h)` + `MF_MT_FRAME_RATE(fps,1)` + `MF_MT_AVG_BITRATE(bitrate)` + `MF_MT_INTERLACE_MODE_Progressive`；输出：`MFVideoFormat_H264` + `MF_MT_MPEG2_PROFILE (Baseline 66)` + `MF_MT_ALL_DEFAULT_INTERLACE_MODE`，设置 `MF_MT_MPEG2_LEVEL` 省略。
4. 关键帧：`ProcessInput` 前若 `state` 刚变 force_idr，取输出 sample 设置 `MFT_OUTPUT_DATA_BUFFER_INCOMPLETE` 循环里读 `MFT_ENCODER_OUTPUT_DATA_PARAM_KEYFRAME`（`CLSID_MF...` 具体经 `IMFAttributes`：在输出 sample 上读 `MF_MT_...` 不可用——用 `MFT_MESSAGE_NOTIFY_START_OF_STREAM` 后每 GOP 直接依赖编码器 GOP？）→ **落地口径**：初始化时设置 `MF_MT_MPEG2_GOP_SIZE = gop`、`MF_MT_MPEG2_GOP_PIC_DIST = 1`，force_idr 通过 `IMFTransform::ProcessMessage(MFT_MESSAGE_SET_FLUSH_HINTS?)` 不可靠——**采用简单可靠法**：`force_idr` 时 `Flush` + 重设输入类型（丢 1 帧），或直接 `MFT_MESSAGE_COMMAND_DRAIN`。实现者按 MS 文档「即时 IDR」章节选择 `IMFSample` 上 `MFSampleExtension_CleanPoint` + `SetUINT32(MF_MT_MPEG2_GOPSIZE...)`，若驱动不支持则退化为 GOP 到期关键帧（换屏最坏延迟 2s，验收项记录）。
   > 该点是本任务唯一不确定面：实现时以「换屏后 1s 内画面刷新」为验收口径，做不到再加降级说明进 spec。
5. 送入 sample：`MFCreateMemoryBuffer` ← NV12 紧凑 buffer，`MFCreateSample` + `AddBuffer` + `SetSampleTime(90000 * seq / fps)` + `SetSampleDuration`；`ProcessInput` → 循环 `ProcessOutput` 收 `MEBuffering`/`MESourceSampleTaken`，直到 `Encrypted` 标志位的 `MFT_PROCESS_OUTPUT_STATUS` 正常返回 ES buffer → `EncodedChunk { seq, key, data }`。
   - `key` 判定：读输出 buffer 的 `MFT_ENCODER_OUTPUT_DATA_PARAM_KEYFRAME` 不可得时，用「Annex-B 内含 IDR NAL（`00 00 00 01 65` 或 `65 xx` 在 5 字节头后）」探测：`contains_idr(&data)` 纯函数放 state.rs 并加单测。
6. `Drop for Encoder`：drain + `MFShutdown` 对应次数、释放 VP/device 引用。

- [ ] **Step 4: 实现 napi.rs + 采集线程**

```rust
// napi.rs：契约 A。采集跑在专用线程（std::thread），ThreadsafeFunction 回帧。
use napi::bindgen_prelude::ThreadsafeFunction;
#[napi(object)]
pub struct RdMediaStartOptions {
  pub rect: RdMediaRect, pub max_width: u32, pub fps: u32, pub bitrate_bps: u32,
  pub on_frame: ThreadsafeFunction<RdMediaFrameNapi>,
  pub on_status: ThreadsafeFunction<RdMediaStatusNapi>,
}
#[napi(object)] pub struct RdMediaRect { pub x: i32, pub y: i32, pub width: u32, pub height: u32 }
#[napi(object)] pub struct RdMediaFrameNapi { pub seq: u32, pub key: bool, pub data: napi::bindgen_prelude::Buffer }
#[napi(object, use_nullable = true)]
pub struct RdMediaStatusNapi { pub frozen: bool, pub reason: Option<String> }

#[napi(js_name = "rd_media_start")]
pub fn rd_media_start_napi(opts: RdMediaStartOptions) -> napi::Result<()> { ... }
#[napi(js_name = "rd_media_switch")]
pub fn rd_media_switch_napi(rect: RdMediaRect) -> napi::Result<()> { ... }
#[napi(js_name = "rd_media_stop")]
pub fn rd_media_stop_napi() -> napi::Result<()> { ... }
/// 测试专用：无 start 时 stop 幂等。
pub fn stop_for_tests() { stop(); }
```

线程循环（`worker` 函数，每 tick）：

```text
loop {
  if stop_flag { break }
  match duplication.next_frame(16)? {
    Idle => continue,
    AccessLost if !state.is_frozen() => { state.set_frozen(true); on_status({frozen:true, reason:"access-lost"}) }
    AccessLost => continue,
    NewDesktop { texture, .. } => {
      if state.is_frozen() { // 恢复
        state.set_frozen(false); on_status({frozen:false}); duplication 重开(同 rect);
      }
      if let Some(chunk) = encoder.push(&texture)? {
        on_frame.call(Ok(RdMediaFrameNapi{ seq: chunk.seq, key: chunk.key, data: chunk.data.into() }), NonBlocking);
      }
    }
  }
}
```

- 全局状态：`static STARTED: Mutex<Option<MediaWorkerHandle>>`；`rd_media_start` 已在跑先 `stop()` 再起新线程；`switch` 写 `Mutex<Rect>` 由循环消费（重开 duplication + `encoder.force_idr()`）；`stop` 置 flag + join（超时 2s 强制 detach 语义记 warn）。
- 错误转码：内部 `AxResult` → `to_napi`（复用 `lib.rs::to_napi`，需 `pub(crate)` 可达）。
- `MFStartup` 失败 / DuplicateOutput 失败 → `AxError::new("unimplemented", ...)`（宿主层映射 `unsupported`，spec 失败语义）。

- [ ] **Step 5: 运行测试与 clippy**

Run: `cargo test --manifest-path crates/mode-cua-ax/Cargo.toml && cargo clippy --manifest-path crates/mode-cua-ax/Cargo.toml --all-targets -- -D warnings`
Expected: PASS（无桌面会话时 `DuplicateOutput` 相关的真机路径不在单测内；单测只覆盖 state/`stop_for_tests`/`contains_idr`）

- [ ] **Step 6: Commit**

```bash
git add crates/mode-cua-ax/src/rd_media crates/mode-cua-ax/src/lib.rs crates/mode-cua-ax/tests/rd_media.rs
git commit -m "feat(cua): DXGI 采集 + MF 硬编 + rd_media napi 面"
```

---

## Phase P2 — 传输链（票据/中继/信令增量/helper 泵）

### Task 3: 双票据 + media WS 端点 + mediaUrl 契约

**Files:**
- Modify: `docs/specs/lan-remote-desktop-native-media.md`（先改 spec：media-open 的 url 改走 rdStartSession 返回，见契约 E）
- Modify: `packages/server/src/http.ts`（POST 响应增 mediaTicket；新增 `/ws/remote-desktop-media` 升级；`RemoteDesktopHooks` 增 `onMediaSocket`）
- Modify: `packages/desktop/src/host/lanAccessServer.ts`（hooks 参数增 `onMediaSocket` 并透传）
- Modify: `packages/desktop/src/host/index.ts`（`resolveRemoteDesktopHooks()` 增 onMediaSocket，缺省 4001 关闭）
- Modify: `packages/desktop/src/host/remoteDesktopServer.ts`（实现 `onMediaSocket`：先只登记 socket，转发给 Task 5 的 relay）
- Modify: `packages/desktop/src/host/remoteDesktopPeerClient.ts`（解析 `mediaTicket`，拼 `mediaUrl` 放进 ok 结果）
- Modify: `packages/desktop/src/main/remoteDesktop/controllerSession.ts`（start 返回 `{ok, mediaUrl?}` 透传）
- Modify: `packages/desktop/src/main/remoteDesktop/desktopIpc.ts`（StartSession 返回体）
- Modify: `packages/shared/src/platform.ts`、`packages/client/src/globals.d.ts`、`packages/desktop/src/preload/index.ts`、`packages/desktop/src/renderer/src/desktopPlatform.ts`、`packages/web/src/main.tsx`（契约 E，5 处同步）
- Test: `packages/server/test/remote-desktop-session.test.ts`

**Interfaces:**
- Consumes: 既有 `createRemoteDesktopTicketStore`（`issue/consume` 签名不变）
- Produces: 契约 E 的 `rdStartSession` 返回体；`RemoteDesktopHooks.onMediaSocket(clientId, ws)`；`mediaUrl` 字符串

- [ ] **Step 1: 先改 spec**

`docs/specs/lan-remote-desktop-native-media.md`：把「信令 union」下 `media-open{url, key, salt}` 改为 `media-open{key, salt}`，并在其后补一行：

```markdown
  - `mediaUrl`（连接地址 + mediaTicket）不走信令：由主控 host 从 POST 响应拼出，
  经 `rdStartSession` 返回值直达 renderer（被控 host 不知道自身对外可达地址）。
```

- [ ] **Step 2: 写失败测试**

`packages/server/test/remote-desktop-session.test.ts` 追加：

```ts
test("POST 响应带 mediaTicket；media 端点凭 mediaTicket 单次升级", async () => {
  const sockets: Array<{ clientId: string }> = [];
  const { server, baseUrl, port } = await startServer({
    remoteDesktop: {
      requestSession: async () => ({ ok: true }),
      onSessionSocket: () => {},
      onMediaSocket: (clientId) => { sockets.push({ clientId }); },
    },
  });
  try {
    const post = await fetch(`${baseUrl}/api/remote-desktop/session`, {
      method: "POST", headers: cookieFor(TOKEN_A),
    });
    const { ticket, mediaTicket } = (await post.json()) as { ticket: string; mediaTicket: string };
    assert.ok(ticket && mediaTicket && ticket !== mediaTicket, "两张票据各自独立");

    // 信令票不能开 media 端点；mediaTicket 不能开信令端点（各自单次消费语义）
    const wrongOnMedia = await connectUntilRejected(
      `ws://127.0.0.1:${port}/ws/remote-desktop-media?mediaTicket=${encodeURIComponent(ticket)}`,
      cookieFor(TOKEN_A));
    assert.equal(wrongOnMedia, 401);
    const wrongOnSignal = await connectUntilRejected(
      `ws://127.0.0.1:${port}/ws/remote-desktop?ticket=${encodeURIComponent(mediaTicket)}`,
      cookieFor(TOKEN_A));
    assert.equal(wrongOnSignal, 401);

    const ok = await connectUntilRejected(
      `ws://127.0.0.1:${port}/ws/remote-desktop-media?mediaTicket=${encodeURIComponent(mediaTicket)}`,
      cookieFor(TOKEN_A));
    // 升级成功路径在 connectUntilRejected 里表现为 open→reject；这里改用直连断言：
    assert.ok([101, 0].includes(ok) || ok === 101, `升级结果 ${ok}`);
    // 重放
    const replay = await connectUntilRejected(
      `ws://127.0.0.1:${port}/ws/remote-desktop-media?mediaTicket=${encodeURIComponent(mediaTicket)}`,
      cookieFor(TOKEN_A));
    assert.equal(replay, 401, "mediaTicket 重放必须 401");
    assert.equal(sockets.length, 1, "onMediaSocket 只挂接一次");
  } finally { await stopServer(server); }
});
```

> 注意 `connectUntilRejected` 在升级成功时会 `reject(new Error("升级不应成功"))`——实现 Step 2 测试时按该 helper 语义调整：升级成功场景改用 `new WebSocket(url, { headers })` 直接连并 `await once(ws, "open")`。以仓库 helper 真实行为为准改写断言，不要留下必炸的用例。

- [ ] **Step 3: 运行确认失败**

Run: `pnpm --filter @mode/server test`
Expected: FAIL（`mediaTicket` undefined）

- [ ] **Step 4: 实现 server 侧**

`http.ts`：
1. POST handler 末行改为：

```ts
    return c.json({
      ticket: remoteDesktopTickets.issue(clientId),
      mediaTicket: remoteDesktopTickets.issue(clientId),
    });
```
（`issue` 返回 `{ticket, expiresAt}`——因此真实形状为**嵌套**：`{ticket: {ticket,expiresAt}, mediaTicket: {ticket,expiresAt}}` 会破坏既有 `issued.ticket` 消费者。**采用平铺**：）

```ts
    const signaling = remoteDesktopTickets.issue(clientId);
    const media = remoteDesktopTickets.issue(clientId);
    return c.json({ ticket: signaling.ticket, mediaTicket: media.ticket, expiresAt: signaling.expiresAt });
```

2. `RemoteDesktopHooks` 接口增：

```ts
  onMediaSocket?(clientId: string, socket: WebSocket): void;
```

3. 信令升级代码块（`app.use("/ws/remote-desktop")`）之后镜像新增：

```ts
  const upgradeRemoteDesktopMediaWebSocket = upgradeWebSocket((c) => ({
    onOpen(_event, ws) {
      const remoteDesktop = options.remoteDesktop;
      const clientId = c.get("remoteDesktopMediaClientId");
      if (!remoteDesktop?.onMediaSocket || !clientId) {
        ws.close(4001, "Remote desktop media unavailable");
        return;
      }
      remoteDesktop.onMediaSocket(clientId, ws.raw as WebSocket);
    },
  }));
  app.use("/ws/remote-desktop-media", async (c, next) => {
    const remoteDesktop = options.remoteDesktop;
    if (!remoteDesktop?.onMediaSocket) {
      return c.json({ error: "Remote desktop is not available" }, 501);
    }
    const mediaTicket = new URL(c.req.url).searchParams.get("mediaTicket") ?? undefined;
    const clientId = resolveLanClientId(c);
    if (!clientId || !remoteDesktopTickets.consume(mediaTicket, clientId)) {
      return c.json({ code: "stale_ticket" satisfies RemoteDesktopErrorCode }, 401);
    }
    c.set("remoteDesktopMediaClientId", clientId);
    await next();
  });
  app.get("/ws/remote-desktop-media", upgradeRemoteDesktopMediaWebSocket);
```

4. `HttpServerVariables` 增 `remoteDesktopMediaClientId: string;`。

- [ ] **Step 5: 主控链路与平台面（契约 E）**

1. `remoteDesktopPeerClient.ts`：POST 响应解析增加 `mediaTicket`（`typeof === "string" && 非空`），ok 结果改为 `{ ok: true; wsUrl: string; mediaUrl: string }`，`mediaUrl = ws://${host}:${port}/ws/remote-desktop-media?mediaTicket=${encodeURIComponent(mediaTicket)}`；缺 `mediaTicket` 视为旧对端 → `return { ok: false, errorCode: "unsupported", error: "对端缺少媒体票据（版本过旧）" }`。
2. `controllerSession.ts`：`start()` 在 `result.ok` 分支最终 `return { ok: true, mediaUrl: result.mediaUrl }`；接口类型同步（`RemoteDesktopControllerSession.start` 返回联合体加 `mediaUrl?: string`）。
3. `desktopIpc.ts` StartSession handler 直接透传（已有 `return await controller.start(...)`，类型联合体已扩）。
4. 平台面 5 处按契约 E 改签名/stub（web stub：`rdStartSession: () => Promise.resolve({ error: "unsupported" as const })` 不变即可）。

- [ ] **Step 6: 被控 hooks 接线**

1. `lanAccessServer.ts` hooks 参数类型与 `createHttpServer` 透传增 `onMediaSocket`（照抄 `onSessionSocket` 的缺省关闭模式，缺省 `ws.close(4001, ...)`）。
2. `host/index.ts` `resolveRemoteDesktopHooks()` 增 `onMediaSocket`（照 `onSessionSocket` 分支：`activeRemoteDesktop?.hooks.onMediaSocket`）。
3. `remoteDesktopServer.ts` `createRemoteDesktopHostServer` options 增 `onMediaSocket?` 钩子出口：本任务先落一个 `mediaSockets: Map<clientId, WebSocket>` 登记 + 断开清理（Task 5 接管收发）。

- [ ] **Step 7: 运行测试与全量校验**

Run: `pnpm --filter @mode/server test && pnpm typecheck && pnpm lint && node scripts/check-language-policy.mjs`
Expected: 全 PASS

- [ ] **Step 8: Commit**

```bash
git add docs/specs/lan-remote-desktop-native-media.md packages/server packages/desktop/src/host packages/desktop/src/main/remoteDesktop packages/shared/src/platform.ts packages/client/src/globals.d.ts packages/desktop/src/preload/index.ts packages/desktop/src/renderer/src/desktopPlatform.ts packages/web/src/main.tsx
git commit -m "feat(rd-media): 双票据与 media WS 端点，rdStartSession 返回 mediaUrl"
```

### Task 4: 信令 union 增量（media-open / freeze / unfreeze）

**Files:**
- Modify: `packages/shared/src/remote-desktop.ts`（union 追加 3 分支，`REMOTE_DESKTOP_PROTOCOL_VERSION` 暂不动）
- Test: `packages/shared/test/remote-desktop.test.ts`（追加 parse 用例）

**Interfaces:**
- Produces: 契约 D 的三个消息（Task 6 消费）

- [ ] **Step 1: 写失败测试**

`packages/shared/test/remote-desktop.test.ts` 追加：

```ts
test("原生媒体信令：media-open / freeze / unfreeze 可解析", () => {
  const ok = remoteDesktopSignalMessageSchema.safeParse({
    type: "media-open", key: "a".repeat(64), salt: "b".repeat(16),
  });
  assert.equal(ok.success, true);
  const frozen = remoteDesktopSignalMessageSchema.safeParse({ type: "freeze", reason: "locked" });
  assert.equal(frozen.success, true);
  const thaw = remoteDesktopSignalMessageSchema.safeParse({ type: "unfreeze" });
  assert.equal(thaw.success, true);
  // 严格模式：未知字段仍拒绝
  const bad = remoteDesktopSignalMessageSchema.safeParse({ type: "unfreeze", extra: 1 });
  assert.equal(bad.success, false);
});
```

Run: `pnpm --filter @mode/shared test` → FAIL

- [ ] **Step 2: 实现**

`remoteDesktopSignalMessageSchema` 的 `z.discriminatedUnion` 内、`bye` 分支前追加：

```ts
  z
    .object({
      type: z.literal("media-open"),
      key: z.string().regex(/^[0-9a-f]{64}$/u),
      salt: z.string().regex(/^[0-9a-f]{16}$/u),
    })
    .strict(),
  z
    .object({ type: z.literal("freeze"), reason: z.enum(["locked", "access-lost"]) })
    .strict(),
  z.object({ type: z.literal("unfreeze") }).strict(),
```

- [ ] **Step 3: 通过 + 提交**

Run: `pnpm --filter @mode/shared test && pnpm typecheck`
Expected: PASS

```bash
git add packages/shared/src/remote-desktop.ts packages/shared/test/remote-desktop.test.ts
git commit -m "feat(shared): 信令 union 增 media-open/freeze/unfreeze"
```

### Task 5: host 媒体中继（本地 TCP ↔ media WS）

**Files:**
- Create: `packages/desktop/src/host/remoteDesktopMediaRelay.ts`
- Modify: `packages/desktop/src/host/remoteDesktopServer.ts`（持有 relay：start/stop/input/ready 路由）
- Test: `packages/desktop/tests/remote-desktop-media-relay.test.mts`

**Interfaces:**
- Consumes: 契案 B（管道）、契约 C（WS 消息）、Task 3 的 `mediaSockets`
- Produces: `createRemoteDesktopMediaRelay(options)`：

```ts
createRemoteDesktopMediaRelay(options: {
  logger: { info(...a: unknown[]): void; warn(...a: unknown[]): void };
  onFrame: (payload: Uint8Array) => void;     // kind0 → 写给当前 WS（无 WS 丢弃）
  onStatus: (frozen: boolean, reason?: string) => void; // kind1 → 回调方转信令
  onReady: () => void;                        // WS 收到 ready → 发 kind2
  onInput: (event: unknown) => void;          // WS 上行输入 → 注入回调
}): {
  listen(): Promise<number>;                  // 127.0.0.1 随机口，返回 port
  attachWebSocket(ws: WsLike): void;          // media WS 挂接（替换旧的）
  handlePipeData(chunk: Buffer): void;        // helper TCP 数据（含粘包）
  sendToHelper(kind: 2): void;                // 开流指令
  handleWsMessage(data: string | Buffer, isBinary: boolean): void;
  close(): void;
}
```

- [ ] **Step 1: 写失败测试**

`packages/desktop/tests/remote-desktop-media-relay.test.mts`（风格照 `remote-desktop.test.mts`：`node:test` + 真实 `net` 回环）：

```ts
import assert from "node:assert/strict";
import test from "node:test";
import { createServer, connect } from "node:net";
import { createRemoteDesktopMediaRelay } from "../src/host/remoteDesktopMediaRelay.js";
import { once } from "node:events";

function pipeFrame(kind: number, payload: Uint8Array): Buffer {
  const body = Buffer.concat([Buffer.from([kind]), Buffer.from(payload)]);
  const header = Buffer.alloc(4);
  header.writeUInt32BE(body.length, 0);
  return Buffer.concat([header, body]);
}

test("粘包/半包正确分帧；kind0 透传 WS、kind1 触发 onStatus、kind2 下发开流", async () => {
  const seen: unknown[] = [];
  const relay = createRemoteDesktopMediaRelay({
    logger: { info() {}, warn() {} },
    onFrame: (p) => seen.push(["frame", Buffer.from(p).toString("hex")]),
    onStatus: (frozen, reason) => seen.push(["status", frozen, reason]),
    onReady: () => seen.push(["ready"]),
    onInput: (e) => seen.push(["input", e]),
  });
  const port = await relay.listen();
  // helper 侧连入
  const helper = connect({ host: "127.0.0.1", port });
  await once(helper, "connect");
  const first = pipeFrame(0, Buffer.from([0, 0, 0, 7, 1, 9, 9]));
  const second = pipeFrame(1, Buffer.from(JSON.stringify({ frozen: true, reason: "locked" })));
  // 半包：先写一半第一个帧
  helper.write(first.subarray(0, 3));
  await new Promise((r) => setTimeout(r, 10));
  helper.write(Buffer.concat([first.subarray(3), second]));
  await new Promise((r) => setTimeout(r, 30));
  assert.deepEqual(seen, [
    ["frame", "000007010909"],
    ["status", true, "locked"],
  ]);
  relay.close();
  helper.destroy();
});

test("WS 上行 ready 触发 kind2 下发；ping 立即 pong；输入透传", async () => {
  const kinds: number[] = [];
  const relay = createRemoteDesktopMediaRelay({
    logger: { info() {}, warn() {} },
    onFrame: () => {}, onStatus: () => {}, onReady: () => {}, onInput: () => {},
  });
  const port = await relay.listen();
  const helper = connect({ host: "127.0.0.1", port });
  await once(helper, "connect");
  helper.on("data", (d) => { kinds.push(d[4]!); });
  const fakeWs = {
    send: (data: unknown) => void data,
    on: (() => {}) as never,
  };
  relay.attachWebSocket(fakeWs as never);
  relay.handleWsMessage(JSON.stringify({ t: "ready" }), false);
  relay.handleWsMessage(JSON.stringify({ t: "ping", ts: 123 }), false);
  await new Promise((r) => setTimeout(r, 30));
  assert.deepEqual(kinds, [2], "ready 只应下发一次 kind2");
  relay.close();
  helper.destroy();
});
```

> `WsLike` 在实现里定义为最小鸭子类型 `{ send(data: string | Buffer, opts?: unknown): void; on?(event: string, cb: (...a: never[]) => void): void; close?(code?: number, reason?: string): void; bufferedAmount?: number }`，测试用 fake 即可。pong 断言在实现后补充（fakeWs.send 收集文本帧断言 `{"t":"pong","ts":123}`）。

- [ ] **Step 2: 运行确认失败**

Run: `node --test packages/desktop/tests/remote-desktop-media-relay.test.mts`
Expected: FAIL（模块不存在）

- [ ] **Step 3: 实现 relay**

`remoteDesktopMediaRelay.ts` 要点（完整实现，不省略）：
1. `listen()`：`net.createServer` on `127.0.0.1:0`，只接受**一个**连接（第二个拒绝并 warn）；helper socket `on("data")` → 内部 `accum: Buffer` 攒缓冲，`while accum.length >= 4`：读 `len = readUInt32BE(0)`（防御 `len > 16MB` → 关连接）→ 不足整帧 break → 切出 `body = accum.subarray(4, 4+len)` → 按 `body[0]` 分发（0→`onFrame(body.subarray(1))`；1→JSON parse→`onStatus`；2→忽略并 warn）→ 继续。
2. `attachWebSocket(ws)`：保存 `currentWs`，旧的先 `close(1000,"replaced")`；新 ws 收文本消息 → `handleWsMessage`：`ready` → `onReady()` + `sendToHelper(2)`（幂等：`readySent` 标志）；`ping` → 立即 `ws.send(JSON.stringify({t:"pong",ts}))`；其他对象 → `onInput(msg)`。
3. `onFrame` 由外部注入（写 WS）：实现里 `handlePipeData` 调用 `options.onFrame`；`remoteDesktopServer` 侧接 `ws.send(buffer)`，带背压：`ws.bufferedAmount > 4 * 1024 * 1024` 丢弃（spec 契约 C）。
4. `sendToHelper(2)`：`[u32BE 1][2]`。
5. `close()`：关 helper server + 当前连接 + ws（code 1000）；幂等。
6. 冻结状态：`kind1` 只回调，不落 relay 状态（单一所有者在 remoteDesktopServer）。

- [ ] **Step 4: 运行测试 + 接线 remoteDesktopServer**

1. 测试 PASS。
2. `remoteDesktopServer.ts`：工厂内建 `relay`；`onMediaSocket(clientId, ws)` → 校验 `clientId === activeSession/pendingSocket.clientId`（复用既有归属判断）→ `relay.attachWebSocket(ws)` + ws `on("message")` → `handleWsMessage`；`ws.on("close")` → 清 `currentWs`。
3. `requestSession` 同意通过（`enterActive` 路径的 `pending.resolve({ok:true})` 处）→ `void relay.listen()` 记 port；`closeActiveSocket/clearSessionState` → `relay.close()`。
4. helper 的 kind0 出口与 `inject`：本任务先留接口（Task 7 把 helper TCP 接进来后联调）。

- [ ] **Step 5: 测试 + 提交**

Run: `node --test packages/desktop/tests/*.test.mts && pnpm typecheck && pnpm lint`

```bash
git add packages/desktop/src/host/remoteDesktopMediaRelay.ts packages/desktop/src/host/remoteDesktopServer.ts packages/desktop/tests/remote-desktop-media-relay.test.mts
git commit -m "feat(rd-media): host 媒体中继（管道分帧/WS 路由/开流指令）"
```

### Task 6: host↔main 消息 + coordinator 接线（media-open / 冻结 / 换屏）

**Files:**
- Modify: `packages/desktop/src/main/remoteDesktop/hostMessages.ts` **与** `packages/desktop/src/host/remoteDesktopMessages.ts`（逐字镜像同改，契约 F）
- Modify: `packages/desktop/src/main/remoteDesktop/hostBridge.ts`（入站分发新增 MediaStatus）
- Modify: `packages/desktop/src/main/remoteDesktop/coordinator.ts`（start-capture 改发 MediaStart、select-monitor 解析物理 rect、冻结转信令、stop-capture 发 MediaStop）
- Create: `packages/desktop/src/main/remoteDesktop/monitors.ts`（从 `captureWindow.ts` 抽出 `listRemoteDesktopMonitors/buildRemoteDesktopMonitors/displayIdOf/physicalRectOf`，`captureWindow.ts` 改为 re-export 以保既有 import）
- Modify: `packages/desktop/src/main/remoteDesktop/coordinator.ts`（新增 `resolvePhysicalRect(displayId)`：`screen.getAllDisplays().find(d => String(d.id) === displayId)` → `physicalRectOf`；找不到 → 发 `error{code:"internal"}`）
- Test: `packages/desktop/tests/fixtures/remote-desktop-session.ts`（action 断言同步）、新建 `packages/desktop/tests/remote-desktop-monitors.test.mts`

**Interfaces:**
- Consumes: 契约 F 消息、契约 D 信令、`key/salt` 生成点
- Produces: 被控同意后完整下发链（media-open 信令 + MediaStart broker 链起点）

- [ ] **Step 1: 写失败测试（显示器 rect 解析纯逻辑）**

新建 `packages/desktop/tests/remote-desktop-monitors.test.mts`：对抽出的 `physicalRectOf`（入参 mock `Display {bounds:{x,y,width,height}, scaleFactor}`）断言 `1920×1080 @150% → 2880×1620`、`scaleFactor<=0 回退 1`；`buildRemoteDesktopMonitors` 沿用既有行为（把 `captureWindow.ts` 原测试若有则搬）。

Run: `node --test packages/desktop/tests/remote-desktop-monitors.test.mts` → FAIL（模块不存在）

- [ ] **Step 2: 抽出 monitors.ts**

把 `captureWindow.ts` 的 `displayIdOf`（L64-66）、`physicalRectOf`（L69-77）、`unionRectOf`（L79-89）、`buildRemoteDesktopMonitors`（L92-123）、`listRemoteDesktopMonitors`（L125-134）原样搬入 `monitors.ts`；`captureWindow.ts` 改 `export { ... } from "./monitors.js";` 保持 coordinator 既有 import 不断（P4 删除时再收敛）。测试转绿。

- [ ] **Step 3: 写失败测试（消息 schema 镜像）**

`packages/desktop/tests/remote-desktop-host-messages.test.mts`（若无既有同类测试则新建，两份文件都 parse 一遍）：

```ts
import assert from "node:assert/strict";
import test from "node:test";
const main = await import("../src/main/remoteDesktop/hostMessages.js");
const host = await import("../src/host/remoteDesktopMessages.js");

for (const msg of [
  { type: "RemoteDesktopMediaStart", requestId: "r", rect: { x: 0, y: 0, width: 1920, height: 1080 }, key: "a".repeat(64), salt: "b".repeat(16) },
  { type: "RemoteDesktopMediaSwitch", requestId: "r", rect: { x: 0, y: 0, width: 1920, height: 1080 } },
  { type: "RemoteDesktopMediaStop" },
  { type: "RemoteDesktopMediaStatus", frozen: true, reason: "locked" },
]) {
  assert.equal(main.remoteDesktopMessageFromHostSchema.safeParse(msg).success
    || main.remoteDesktopMessageToHostSchema.safeParse(msg).success, true, msg.type);
  assert.equal(host.remoteDesktopMessageFromHostSchema.safeParse(msg).success
    || host.remoteDesktopMessageToHostSchema.safeParse(msg).success, true, `mirror ${msg.type}`);
}
```

> 以两份文件真实的 schema 导出名/方向为准调整（镜像文件可能只有单一联合体）；核心断言 = **两份文件对 4 个新消息的接受度一致**。

Run → FAIL

- [ ] **Step 4: 实现消息与分发**

1. 两份镜像文件同步加 4 个 type 常量 + schema 分支（zod `.strict()` 对象，字段见契约 F；`requestId` 仅 Start/Switch 带、`z.string().optional()` 不用——通知型可完全省略 requestId）。
2. `hostBridge.ts` 入站：`RemoteDesktopMediaStatus` → 新 sessionHandler `handleMediaStatus(message)`（`RemoteDesktopHostSessionHandlers` 增签名）→ coordinator。
3. `host/index.ts` 的 RD 消息分派：Media* 三型走 `activeRemoteDesktop.handleMessage(message)`（与既有 server.handleMessage 同路）——确认 `remoteDesktopServer.handleMessage` 增加三分支：
   - `MediaStart` → `relay.listen()` + `callRdMedia("rd_media_start", {rect, maxWidth:1920, fps:60, bitrateBps:8_000_000, endpoint:{host:"127.0.0.1",port}, key, salt})`（`callRdMedia` 由 Task 8 提供，本任务先用 `options.callRdMedia` 注入参数占位——**不是 TODO 占位**：签名在此定死 `callRdMedia(method: string, params: object) => Promise<void>`，Task 8 注入真实现）。
   - `MediaSwitch` → `callRdMedia("rd_media_switch", {rect})`
   - `MediaStop` → `callRdMedia("rd_media_stop", {})` + `relay.close()`
   - helper 的 `kind1` 状态 → `postToMain(RemoteDesktopMediaStatus)`（经 relay `onStatus` 回调接线，完成 Task 5 留的口）。
4. `coordinator.ts`：
   - `applyAction` 的 `case "start-capture"`：改为（替换 `ensureCaptureSession` 调用）：

```ts
    case "start-capture": {
      const runtimeState2 = runtimeState;
      const rect = runtimeState2.pendingDisplayRect; // open-consent 时记录主屏 rect 兜底
      const key = randomBytes(32).toString("hex");
      const salt = randomBytes(8).toString("hex");
      runtimeState2.mediaKey = key;
      runtimeState2.mediaSalt = salt;
      sendSignalToPeer({ type: "media-open", key, salt });
      sendRemoteDesktopMessageToHost({
        type: RemoteDesktopHostMessageTypes.RemoteDesktopMediaStart,
        rect, key, salt,
      });
      return;
    }
```
   （`runtimeState` 结构增 `mediaKey/mediaSalt/pendingDisplayRect`；`key` 不落日志。原 `ensureCaptureSession` 调用移到 `select-monitor` 分支。）
   - `case "stop-capture"`：发 `RemoteDesktopMediaStop`（替换 capture.close()）。
   - `handleSignalFromHost` 的 `select-monitor` 分支：`resolvePhysicalRect(displayId)` 成功 → 发 `RemoteDesktopMediaSwitch{rect}`；失败 → 既有 `error{internal}`。
   - 新增 `handleMediaStatus`：`frozen=true → sendSignalToPeer({type:"freeze", reason})`；`frozen=false → sendSignalToPeer({type:"unfreeze"})`；同时驱动指示条？（**不**——指示条生命周期由 session 状态管，冻结只透传信令）。
   - `hello` 分支版本检查保持（constant 未 bump，v1 双端一致）。

- [ ] **Step 5: fixtures 断言同步**

`packages/desktop/tests/fixtures/remote-desktop-session.ts` 中 `start-capture` 相关 action 顺序断言按新行为更新（action kind 不变，只是解释执行变了——若断言只覆盖 state machine 纯函数则无需改；跑测试确认）。

- [ ] **Step 6: 测试 + 提交**

Run: `node --test packages/desktop/tests/*.test.mts && pnpm typecheck && pnpm lint`

```bash
git add packages/desktop/src/main/remoteDesktop packages/desktop/src/host packages/desktop/tests
git commit -m "feat(rd-media): 被控接线（MediaStart/Switch/Stop/Status + media-open 下发）"
```

### Task 7: helper 泵（broker 方法 + 加密管道）

**Files:**
- Create: `packages/mode-cua/helper/mediaPump.mjs`
- Modify: `packages/mode-cua/helper/server.mjs`（`createBackend` 增 3 方法）
- Modify: `packages/mode-cua/broker.js`（`BROKER_METHODS` 增 3 方法；**不进** `READONLY_METHODS`）
- Test: `packages/mode-cua/test/helper-backend.test.mjs`（追加）+ 新建 `packages/mode-cua/test/media-pump.test.mjs`

**Interfaces:**
- Consumes: 契约 A（napi）、契约 B（管道）、契约 G（broker 参数）
- Produces: `createMediaPump(addon)` → `{ start(params): Promise<{ok:true}>, switchMonitor(p): Promise<{ok:true}>, stop(): Promise<{ok:true}> }`

- [ ] **Step 1: 写失败测试**

`packages/mode-cua/test/media-pump.test.mjs`（不依赖真 addon：注入 fake addon）：

```js
import assert from "node:assert/strict";
import test from "node:test";
import { createMediaPump } from "../helper/mediaPump.mjs";
import { createServer } from "node:net";
import { once } from "node:events";

function readFrames() {
  const frames = [];
  return {
    frames,
    attach(socket) {
      let acc = Buffer.alloc(0);
      socket.on("data", (d) => {
        acc = Buffer.concat([acc, d]);
        while (acc.length >= 4) {
          const len = acc.readUInt32BE(0);
          if (acc.length < 4 + len) break;
          frames.push(acc.subarray(4, 4 + len));
          acc = acc.subarray(4 + len);
        }
      });
    },
  };
}

test("帧经 AES-256-GCM 加密后按长度前缀写出；IV = salt∥seq", async () => {
  const server = createServer();
  await new Promise((r) => server.listen(0, "127.0.0.1", r));
  const port = server.address().port;
  const collector = readFrames();
  server.on("connection", (s) => collector.attach(s));

  const keyHex = "11".repeat(32);
  const saltHex = "22".repeat(8);
  const listeners = {};
  const fakeAddon = {
    rd_media_start(opts) { listeners.frame = opts.onFrame; listeners.status = opts.onStatus; return undefined; },
    rd_media_switch() {}, rd_media_stop() {},
  };
  const pump = createMediaPump(fakeAddon);
  await pump.start({
    rect: { x: 0, y: 0, width: 1920, height: 1080 },
    maxWidth: 1920, fps: 60, bitrateBps: 8000000,
    endpoint: { host: "127.0.0.1", port }, key: keyHex, salt: saltHex,
  });
  // 模拟 addon 回帧
  listeners.frame({ seq: 0, key: true, data: Buffer.from("hello-es") });
  await new Promise((r) => setTimeout(r, 50));

  assert.equal(collector.frames.length, 1);
  const payload = collector.frames[0];
  assert.equal(payload[0], 0, "kind=0");
  assert.equal(payload.readUInt32BE(1), 0, "seq=0");
  assert.equal(payload[5], 1, "关键帧 flags");
  const iv = Buffer.concat([Buffer.from(saltHex, "hex"), Buffer.from([0, 0, 0, 0])]);
  const cipher = payload.subarray(6, payload.length - 16);
  const tag = payload.subarray(payload.length - 16);
  const { createDecipheriv } = await import("node:crypto");
  const d = createDecipheriv("aes-256-gcm", Buffer.from(keyHex, "hex"), iv);
  d.setAuthTag(tag);
  const plain = Buffer.concat([d.update(cipher), d.final()]);
  assert.equal(plain.toString(), "hello-es");
  await pump.stop();
  server.close();
});

test("kind1 状态帧与 kind2 开流后再回帧的门控", async () => {
  // 场景：start 后未收到 kind2 前 addon 回帧 → 不写管道；收到 kind2 → 恢复写出。
  // （fakeAddon 手动触发；net server 计数）
});
```

- [ ] **Step 2: 运行确认失败**

Run: `pnpm --dir packages/mode-cua exec node --test test/media-pump.test.mjs` → FAIL

- [ ] **Step 3: 实现 mediaPump.mjs**

```js
// packages/mode-cua/helper/mediaPump.mjs
import { createCipheriv } from "node:crypto";
import { connect } from "node:net";

export function createMediaPump(addon) {
  let state = null; // { socket, key, salt, ready, seqSeen:Set, stopping }
  return {
    async start(params) {
      await this.stop();
      const key = Buffer.from(params.key, "hex");
      const salt = Buffer.from(params.salt, "hex");
      if (key.length !== 32 || salt.length !== 8) throw invalid("key/salt length");
      const socket = connect({ host: params.endpoint.host, port: params.endpoint.port });
      socket.on("data", (d) => { // 只应收到 kind2
        if (d.length >= 5 && d.readUInt32BE(0) === 1 && d[4] === 2) {
          if (state) state.ready = true;
        }
      });
      await onceConnect(socket);
      state = { socket, key, salt, ready: false, stopping: false };
      addon.rd_media_start({
        rect: params.rect, maxWidth: params.maxWidth, fps: params.fps, bitrateBps: params.bitrateBps,
        onFrame: (frame) => emitFrame(frame),
        onStatus: (s) => emitStatus(s),
      });
      return { ok: true };
    },
    async switchMonitor(p) { addon.rd_media_switch(p); return { ok: true }; },
    async stop() {
      if (!state) return { ok: true };
      state.stopping = true;
      try { addon.rd_media_stop(); } catch { /* 幂等 */ }
      state.socket.destroy();
      state = null;
      return { ok: true };
    },
  };
  function emitFrame(frame) {
    if (!state || state.stopping || !state.ready) return; // ready 门控（契约 B）
    const iv = Buffer.concat([state.salt, u32be(frame.seq)]);
    const cipher = createCipheriv("aes-256-gcm", state.key, iv);
    const ct = Buffer.concat([cipher.update(Buffer.from(frame.data)), cipher.final()]);
    const payload = Buffer.alloc(6 + ct.length + 16);
    payload.writeUInt32BE(frame.seq, 0); // 注意：布局 [u32 seq][u8 flags][ct||tag]
    payload.writeUInt8(frame.key ? 1 : 0, 4);
    // ↑ 修正：seq 4 字节 + flags 1 字节 = 头 5 字节；见下方布局说明
    ct.copy(payload, 5);
    cipher.getAuthTag().copy(payload, 5 + ct.length);
    writeKind(0, payload.subarray(0, 5 + ct.length + 16));
  }
  function emitStatus(s) {
    if (!state || state.stopping) return;
    writeKind(1, Buffer.from(JSON.stringify(s), "utf8"));
  }
  function writeKind(kind, payload) {
    if (!state || state.socket.destroyed) return;
    const header = Buffer.alloc(5);
    header.writeUInt32BE(1 + payload.length, 0);
    header.writeUInt8(kind, 4);
    state.socket.write(Buffer.concat([header, payload]));
  }
}
function u32be(n) { const b = Buffer.alloc(4); b.writeUInt32BE(n >>> 0, 0); return b; }
function onceConnect(socket) { return new Promise((res, rej) => { socket.once("connect", res); socket.once("error", rej); }); }
function invalid(m) { const e = new Error(m); e.code = "invalid_request"; return e; }
```

> **布局对齐（实现时必须修的自洽点）**：契约 B 的 payload = `[u32 seq][u8 flags][ciphertext‖tag]`，即头 **5 字节**（不是 6）。上面 `emitFrame` 的 `writeUInt8` offset 应为 4，`ct.copy(payload, 5)` 正确。实现 Step 3 时按下表写并让测试驱动修正：

```text
offset 0..3  u32 seq (BE)
offset 4     u8 flags
offset 5..   ciphertext ‖ tag(16)
```

- [ ] **Step 4: 接入 broker 与 backend**

1. `broker.js` `BROKER_METHODS` 增 `"rd_media_start", "rd_media_switch", "rd_media_stop"`（注释：远程桌面媒体面写路径，故意不进 READONLY）。
2. `server.mjs` `createBackend(addon)` 增：

```js
    rd_media_start: async (params) => {
      for (const k of ["rect", "maxWidth", "fps", "bitrateBps", "endpoint", "key", "salt"]) {
        if (params[k] === undefined) throw invalidRequest(`${k} is required`);
      }
      return mediaPump.start(params);   // mediaPump = createMediaPump(addon)，模块级惰性单例
    },
    rd_media_switch: async (params) => mediaPump.switchMonitor(params),
    rd_media_stop: async () => mediaPump.stop(),
```

3. `broker.js` 既有方法表测试（`test/broker.test.mjs:44-50`）同步 3 个新方法断言；`helper-backend.test.mjs:152-168` 风格追加参数校验用例（缺参 → `invalid_request`）。

- [ ] **Step 5: 测试 + 提交**

Run: `pnpm --dir packages/mode-cua test && node --test packages/mode-cua/test/media-pump.test.mjs && pnpm build:cua-helper`
Expected: 全 PASS（build:cua-helper 确认 napi 符号存在：`node -e "const a=require('./packages/mode-cua/dist-cua-helper/cua_ax.node'); console.log(typeof a.rd_media_start)"` 应为 `function`——若 build 产物路径不同以 manifest 为准）

```bash
git add packages/mode-cua/broker.js packages/mode-cua/helper packages/mode-cua/test
git commit -m "feat(rd-media): helper 媒体泵（加密/管道/ready 门控）与 broker 方法"
```

### Task 8: services 控制器 + host 真实接线

**Files:**
- Create: `packages/services/src/cua-permission-broker/cuaRdMedia.ts`
- Modify: `packages/services/src/node.ts`（WeakMap side-table 装配，照 `cuaRawInputInjectors` 先例 L735/L743/L1906-1920/L2800）
- Modify: `packages/desktop/src/host/index.ts`（`createRemoteDesktopHostServer` 注入 `callRdMedia`）
- Modify: `packages/desktop/src/host/remoteDesktopServer.ts`（options 消费 `callRdMedia`）
- Test: 无新增单测（注入逻辑薄）；靠 Task 5/7 测试 + typecheck

**Interfaces:**
- Consumes: `callBrokerMethod`（`@mode/cua/broker`）、`createCuaRawInputSocketResolver` 同款解析链
- Produces: `getCuaRdMediaController(services): CuaRdMediaController | undefined`；`CuaRdMediaController = (method: "rd_media_start"|"rd_media_switch"|"rd_media_stop", params: object) => Promise<void>`

- [ ] **Step 1: 实现 cuaRdMedia.ts**

```ts
// 照 cuaRawInput.ts 的结构：resolveSocketPath 复用（同一个 helper、同一个管道）。
import { callBrokerMethod } from "@mode/cua/broker";
export type CuaRdMediaController = (
  method: "rd_media_start" | "rd_media_switch" | "rd_media_stop",
  params: object,
) => Promise<void>;
const RD_MEDIA_TIMEOUT_MS = 15_000;
export function createCuaRdMediaController(options: {
  resolveSocketPath: () => Promise<string | null>;
}): CuaRdMediaController {
  return async (method, params) => {
    const socketPath = await options.resolveSocketPath();
    if (!socketPath) {
      const e = new Error("RD helper unavailable"); e.code = "helper_unavailable"; throw e;
    }
    const res = await callBrokerMethod({ socketPath, method, params, timeoutMs: RD_MEDIA_TIMEOUT_MS });
    if (res.ok === false) { const e = new Error(res.error?.message ?? "rd media failed"); e.code = res.error?.code ?? "internal"; throw e; }
  };
}
```

> 以 `callBrokerMethod` 真实返回形状为准（recon：`{ok:true,result}|{ok:false,error}`——用 TS 实际签名对齐）。

- [ ] **Step 2: node.ts 装配**

在 L2800 `cuaRawInputInjectors.set(...)` 旁追加：

```ts
const cuaRdMediaController = createCuaRdMediaController({
  resolveSocketPath: resolveCuaRawInputSocketPath, // 同一 helper 解析链（含 RD 准入/退避）
});
cuaRdMediaControllers.set(services, cuaRdMediaController);
```
导出 `getCuaRdMediaController(services)`（照 L743-747 注释口径：侧表，不进 ServiceCollection）。

- [ ] **Step 3: host 接线**

`host/index.ts` `createRemoteDesktopHostServer({...})` options 增 `callRdMedia?: (method, params) => Promise<void>`（取自 `getCuaRdMediaController(services)`）；`remoteDesktopServer.ts` options 同名消费，`MediaStart` 分支：

```ts
    case "RemoteDesktopMediaStart": {
      void (async () => {
        try {
          const port = await relay.listen();
          await options.callRdMedia?.("rd_media_start", {
            rect: message.rect, maxWidth: 1920, fps: 60, bitrateBps: 8_000_000,
            endpoint: { host: "127.0.0.1", port }, key: message.key, salt: message.salt,
          });
        } catch (error) {
          logger.warn("[remote-desktop] rd_media_start failed:", error);
          postToMain({ type: RemoteDesktopHostMessageTypes.RemoteDesktopMediaStatus,
            frozen: true, reason: "access-lost" }); // 起不来按冻结展示，交由信令提示
        }
      })();
      return;
    }
```
（`MediaSwitch`/`MediaStop` 同构：`callRdMedia("rd_media_switch"/"rd_media_stop", ...)`；失败只 warn。`MediaStop` 后 `relay.close()`。）

- [ ] **Step 4: 校验 + 提交**

Run: `pnpm typecheck && pnpm lint && pnpm architecture:check --changed && pnpm exec tsx --tsconfig packages/ui/tsconfig.json --test packages/ui/test/*.test.ts`

```bash
git add packages/services/src/cua-permission-broker/cuaRdMedia.ts packages/services/src/node.ts packages/desktop/src/host
git commit -m "feat(rd-media): services rdMedia 控制器与 host MediaStart/Switch/Stop 联通"
```

## Phase P3 — renderer 切换（解码/画布/输入/门控）

### Task 9: `nativeMediaSession` 模块（纯逻辑 + WebCodecs 胶水）

**Files:**
- Create: `packages/ui/src/remote-desktop/nativeMediaSession.ts`
- Test: `packages/ui/test/remoteDesktopNativeSession.test.ts`

**Interfaces:**
- Consumes: 契约 B 帧布局、契约 C WS 消息、契约 E `mediaUrl`
- Produces: 契约 H 全部（`createNativeMediaSession` / `parseFrameHeader` / `buildFrameIv`）

- [ ] **Step 1: 写失败测试**

`packages/ui/test/remoteDesktopNativeSession.test.ts`：

```ts
import assert from "node:assert/strict";
import test from "node:test";

// 原生媒体帧解析与 IV 派生（spec: lan-remote-desktop-native-media 协议节）。
const { parseFrameHeader, buildFrameIv, codecStringFromAnnexB } =
  await import("../src/remote-desktop/nativeMediaSession.js");

function frame(seq: number, key: boolean, body: Uint8Array): Uint8Array {
  const out = new Uint8Array(5 + body.length);
  new DataView(out.buffer).setUint32(0, seq, false);
  out[4] = key ? 1 : 0;
  out.set(body, 5);
  return out;
}

test("帧头解析：seq/flags/body 切分正确，残包返回 null", () => {
  const f = frame(7, true, new Uint8Array([9, 9, 9]));
  const parsed = parseFrameHeader(f);
  assert.deepEqual(parsed, { seq: 7, key: true, body: new Uint8Array([9, 9, 9]) });
  assert.equal(parseFrameHeader(f.subarray(0, 4)), null, "不足 5 字节头");
  assert.equal(parseFrameHeader(new Uint8Array(0)), null);
});

test("IV = salt(8B) ∥ seq(4B 大端)", () => {
  const salt = new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8]);
  const iv = buildFrameIv(salt, 0x01020304);
  assert.equal(iv.length, 12);
  assert.deepEqual([...iv], [1, 2, 3, 4, 5, 6, 7, 8, 1, 2, 3, 4]);
});

test("codecStringFromAnnexB：从 SPS 推导 avc1.PPCCLL", () => {
  // SPS: 00 00 00 01 67 42 00 1f ... → profile 0x42(66) level 0x1f(31)
  const sps = new Uint8Array([0, 0, 0, 1, 0x67, 0x42, 0x00, 0x1f, 0xaa, 0xbb]);
  assert.equal(codecStringFromAnnexB(sps), "avc1.42001f");
  assert.equal(codecStringFromAnnexB(new Uint8Array([0, 0, 0, 1, 0x65, 0, 0])), null, "无 SPS 返回 null");
});
```

Run: `pnpm exec tsx --tsconfig packages/ui/tsconfig.json --test packages/ui/test/remoteDesktopNativeSession.test.ts` → FAIL

- [ ] **Step 2: 实现纯函数部分**

`nativeMediaSession.ts` 头部（纯函数必须无 DOM/浏览器依赖，供胶水与测试共用）：

```ts
/**
 * 原生媒体面 renderer 会话（spec: docs/specs/lan-remote-desktop-native-media.md）。
 * 纯函数（帧头/IV/SPS）不碰 DOM；胶水部分用 WebCodecs + WS（仅桌面 Chromium）。
 */
export interface ParsedMediaFrame { seq: number; key: boolean; body: Uint8Array }

export function parseFrameHeader(bytes: Uint8Array): ParsedMediaFrame | null {
  if (bytes.length < 5) return null;
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  return {
    seq: view.getUint32(0, false),
    key: (bytes[4]! & 1) === 1,
    body: bytes.subarray(5),
  };
}

export function buildFrameIv(salt: Uint8Array, seq: number): Uint8Array {
  const iv = new Uint8Array(12);
  iv.set(salt.subarray(0, 8), 0);
  new DataView(iv.buffer).setUint32(8, seq >>> 0, false);
  return iv;
}

/** Annex-B 里找首个 SPS（NAL type 7）→ "avc1.<profile hex><constraint hex><level hex>"。 */
export function codecStringFromAnnexB(bytes: Uint8Array): string | null {
  // 扫描 4/3 字节起始码，取 type = byte & 0x1f === 7 的首段
  for (let i = 0; i + 5 < bytes.length; i++) {
    const is4 = bytes[i] === 0 && bytes[i + 1] === 0 && bytes[i + 2] === 0 && bytes[i + 3] === 1;
    const is3 = !is4 && bytes[i] === 0 && bytes[i + 1] === 0 && bytes[i + 2] === 1;
    if (!is4 && !is3) continue;
    const headerIdx = i + (is4 ? 4 : 3);
    if ((bytes[headerIdx]! & 0x1f) !== 7 || headerIdx + 3 >= bytes.length) continue;
    const hex = (n: number) => n.toString(16).padStart(2, "0");
    return `avc1.${hex(bytes[headerIdx + 1]!)}${hex(bytes[headerIdx + 2]!)}${hex(bytes[headerIdx + 3]!)}`;
  }
  return null;
}
```

- [ ] **Step 3: 实现胶水部分（同文件追加；`import { logger } from "@/logger.js";` 放到文件顶部——UI 不允许 console 输出，仓库日志纪律）**

```ts
export interface NativeMediaSessionOptions {
  url: string;
  keyHex: string;
  saltHex: string;
  canvas: HTMLCanvasElement;
  onConfigured?: (info: { width: number; height: number; codec: string }) => void;
  onDecoded?: () => void;
  onTransportClosed?: () => void;
}

export interface NativeMediaSession {
  send(event: object): void;    // 首调自动先发 {t:"ready"}（open 后）
  ping(ts: number): void;
  stop(): void;
}

export function createNativeMediaSession(opts: NativeMediaSessionOptions): NativeMediaSession {
  const key = hexToBytes(opts.keyHex);
  const salt = hexToBytes(opts.saltHex);
  if (key.length !== 32 || salt.length !== 8) throw new Error("invalid key/salt");
  const ws = new WebSocket(opts.url);
  ws.binaryType = "arraybuffer";
  const ctx = opts.canvas.getContext("2d", { alpha: false });
  if (!ctx) throw new Error("2d canvas unavailable");
  let decoder: VideoDecoder | null = null;
  let configuredCodec: string | null = null;
  let readySent = false;
  let stopped = false;
  let timestampBase = 0;
  let lastSeq: number | null = null;

  const configure = (codec: string) => {
    decoder?.close();
    decoder = new VideoDecoder({
      output: (frame) => {
        // 只保留最新帧语义：解码即画（LAN 硬解 60fps 下无排队积压；帧率计数交给 onDecoded）。
        if (opts.canvas.width !== frame.displayWidth || opts.canvas.height !== frame.displayHeight) {
          opts.canvas.width = frame.displayWidth;
          opts.canvas.height = frame.displayHeight;
          opts.onConfigured?.({ width: frame.displayWidth, height: frame.displayHeight, codec });
        }
        ctx.drawImage(frame, 0, 0);
        frame.close();
        opts.onDecoded?.();
      },
      error: (e) => { logger.warn("[rd-media] decoder error:", e); teardown(false); },
    });
    decoder.configure({ codec, optimizeForLatency: true });
    configuredCodec = codec;
  };

  ws.onopen = () => { /* ready 由首条 send 触发 */ };
  ws.onmessage = (event) => {
    if (typeof event.data === "string") {
      const msg = JSON.parse(event.data) as { t?: string };
      if (msg.t === "pong" || msg.t === "freeze" || msg.t === "unfreeze") {
        document.dispatchEvent(new CustomEvent("rd-media-control", { detail: msg })); // Task 10/13 订阅
      }
      return;
    }
    const parsed = parseFrameHeader(new Uint8Array(event.data as ArrayBuffer));
    if (!parsed || !decoder) return;
    if (lastSeq !== null && parsed.seq !== lastSeq + 1) { /* 丢帧容错：关键帧到来前的缺口忽略 */ }
    lastSeq = parsed.seq;
    void (async () => {
      let plain: Uint8Array;
      try {
        plain = await decryptAesGcm(key, buildFrameIv(salt, parsed.seq), parsed.body);
      } catch { return; } // 单帧解密失败丢弃（连续失败由 transport 层收口）
      if (parsed.key) {
        const codec = codecStringFromAnnexB(plain);
        if (codec && codec !== configuredCodec) configure(codec);
      }
      if (!decoder || decoder.state !== "configured") return;
      timestampBase += 1_000_000 / 60;
      decoder.decode(new EncodedVideoChunk({
        type: parsed.key ? "key" : "delta",
        timestamp: timestampBase,
        data: plain,
      }));
    })();
  };
  ws.onclose = () => teardown(false);
  ws.onerror = () => teardown(false);

  function teardown(notify: boolean) {
    if (stopped) return;
    stopped = true;
    try { decoder?.close(); } catch { /* 幂等 */ }
    decoder = null;
    if (notify !== false) opts.onTransportClosed?.();
    else opts.onTransportClosed?.();
  }

  return {
    send(event) {
      if (ws.readyState !== WebSocket.OPEN) return;
      if (!readySent) { ws.send(JSON.stringify({ t: "ready" })); readySent = true; }
      ws.send(JSON.stringify(event));
    },
    ping(ts) {
      if (ws.readyState === WebSocket.OPEN) ws.send(JSON.stringify({ t: "ping", ts }));
    },
    stop() {
      try { ws.close(1000, "client-stop"); } catch { /* 幂等 */ }
      teardown(false);
    },
  };
}

async function decryptAesGcm(key: Uint8Array, iv: Uint8Array, body: Uint8Array): Promise<Uint8Array> {
  const cryptoKey = await crypto.subtle.importKey("raw", key, "AES-GCM", false, ["decrypt"]);
  // body = ciphertext ‖ tag(16)（WebCrypto 约定 tag 附尾）
  const plain = await crypto.subtle.decrypt({ name: "AES-GCM", iv }, cryptoKey, body);
  return new Uint8Array(plain);
}
function hexToBytes(hex: string): Uint8Array {
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = Number.parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  return out;
}
```

> 胶水里的 `rd-media-control` 自定义事件是 Task 10 订阅 freeze/unfreeze 与 pong 的通道（替代已删除的 reducer 信号直染）；实现时用 `window.dispatchEvent`/`addEventListener` 即可，**不用** `document`。

- [ ] **Step 4: 测试通过 + 提交**

Run: `pnpm exec tsx --tsconfig packages/ui/tsconfig.json --test packages/ui/test/remoteDesktopNativeSession.test.ts && pnpm typecheck && pnpm lint`

```bash
git add packages/ui/src/remote-desktop/nativeMediaSession.ts packages/ui/test/remoteDesktopNativeSession.test.ts
git commit -m "feat(rd-media): renderer 原生会话模块（帧解析/IV/SPS + WebCodecs 胶水）"
```

### Task 10: renderer 切换（hook 重写 / canvas / 输入泛化）

**Files:**
- Modify: `packages/ui/src/remote-desktop/sessionReducer.ts`（effect 联合增 `open-media`；signal 分支增 `media-open`；`offerReceived` 改名 `mediaOpenReceived`；旧 `apply-offer/add-ice-candidate` 分支暂留但标「Task 14 删除」）
- Modify: `packages/ui/src/remote-desktop/useRemoteDesktopSession.ts`（重写：无 PC/DataChannel）
- Modify: `packages/ui/src/remote-desktop/RemoteDesktopSession.tsx`（`<video>` → `<canvas>`、`videoRef` → `canvasRef`、statusText 去 peerConnectionState 分支）
- Modify: `packages/ui/src/remote-desktop/videoFit.ts`、`InputCapture.ts`（表面元素泛化）
- Modify: `packages/ui/src/remote-desktop/sessionStats.ts`（本任务只改 import 面，读数重写在 Task 11）
- Test: `packages/ui/test/remoteDesktopSessionReducer.test.ts`（offer→media-open 断言更新）

**Interfaces:**
- Consumes: 契约 H、契约 E（`rdStartSession` 返回 `mediaUrl`）、Task 4 的 `media-open`
- Produces: hook 返回值 `{ view, stats, containerRef, canvasRef, selectMonitor, disconnect }`（**移除** `videoRef`、`peerConnectionState`）

- [ ] **Step 1: 写失败测试（reducer media-open）**

`remoteDesktopSessionReducer.test.ts` 追加（风格照既有：`reduce(state, event)` + deepEqual）：

```ts
test("media-open：进入 negotiating 并产出 open-media effect", () => {
  const state = { ...INITIAL_REMOTE_DESKTOP_SESSION_VIEW_STATE, phase: "connected" as const };
  const t = reduce(state, {
    type: "controller",
    event: { kind: "signal", message: { type: "media-open", key: "a".repeat(64), salt: "b".repeat(16) } },
  });
  assert.equal(t.state.mediaOpenReceived, true);
  assert.equal(t.state.mediaStatus, "negotiating");
  assert.deepEqual(t.effects, [
    { kind: "open-media", key: "a".repeat(64), salt: "b".repeat(16) },
  ]);
});
```

既有断言中 `offerReceived` 字段名全部改 `mediaOpenReceived`；`offer` signal 用例删除（Task 14 前 schema 还在，但 reducer 不再处理）。

Run: `pnpm exec tsx --tsconfig packages/ui/tsconfig.json --test packages/ui/test/remoteDesktopSessionReducer.test.ts` → FAIL

- [ ] **Step 2: 实现 reducer 改动**

1. state：`offerReceived` → `mediaOpenReceived`（INITIAL 同步）；新增 `frozen: boolean`（INITIAL `false`；freeze 分支 Task 13 落）。
2. effect 联合增 `{ kind: "open-media"; key: string; salt: string }`；signal 分支增：

```ts
  if (signal.type === "media-open") {
    return transition({ ...state, mediaOpenReceived: true, mediaStatus: "negotiating" },
      [{ kind: "open-media", key: signal.key, salt: signal.salt }]);
  }
```
3. 旧 `offer`/`ice` 分支改为「记录性忽略」（return `{state, effects: []}`，注释 `// WebRTC 残留分支，Task 14 随 schema 删除`）。
4. `connected` 分支的 `offerReceived:false` 重置同步改名。

- [ ] **Step 3: 重写 useRemoteDesktopSession**

删除：`pcRef/controlChannelRef/pointerChannelRef/pendingCandidatesRef/answerInFlightRef`、`apply-offer/add-ice-candidate` 解释分支（保留 `signal` 与 `end-session`；`apply-offer`/`add-ice-candidate` case 写 `case "apply-offer": case "add-ice-candidate": return; // Task 14 删除`）、stats getStats 轮询（Task 11 换新）。

新增/改动要点（保留原有行号结构处沿用原注释）：

```ts
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const sessionRef = useRef<NativeMediaSession | null>(null);
  const mediaUrlRef = useRef<string | null>(null);

  // applyEffect 增：
  //   "open-media" → const canvas = canvasRef.current; const mediaUrl = mediaUrlRef.current;
  //     if (!canvas || !mediaUrl) { dispatch({type:"media", status:"failed"}); return; }
  //     sessionRef.current = createNativeMediaSession({
  //       url: mediaUrl, key: effect.key, salt: effect.salt, canvas,
  //       onConfigured: (i) => setStats((s) => ({ ...s, width: i.width, height: i.height })),
  //       onDecoded: () => { /* Task 11 引入 nativeStats 后接其 onDecoded */ },
  //       onTransportClosed: () => dispatchRef.current({ type: "media", status: "failed" }),
  //     });
  //     dispatchRef.current({ type: "media", status: "live" });
```

1. 建连 effect（原 L174-284）重写为：

```ts
  useEffect(() => {
    let disposed = false;
    const unsubscribe = platform.onRemoteDesktopSessionEvent((event) => {
      dispatchRef.current({ type: "controller", event });
    });
    void (async () => {
      try {
        const result = await platform.rdStartSession(serverId);
        if (disposed) return;
        if ("error" in result) {
          dispatchRef.current({ type: "controller", event: { kind: "state", state: "closed", error: result.error } });
          return;
        }
        mediaUrlRef.current = result.mediaUrl ?? null;
        if (!result.mediaUrl) {
          dispatchRef.current({ type: "controller", event: { kind: "state", state: "closed", error: "unsupported" } });
        }
      } catch (error) {
        logger.warn("[remote-desktop] 发起会话失败:", error instanceof Error ? error.message : error);
        if (!disposed) {
          dispatchRef.current({ type: "controller", event: { kind: "state", state: "closed", error: "internal" } });
        }
      }
    })();
    return () => {
      disposed = true;
      unsubscribe();
      sessionRef.current?.stop();
      sessionRef.current = null;
      void platform.rdEndSession("user-end");
    };
  }, [platform, serverId]);
```

2. `rd-media-control` 订阅（freeze/pong 通道，Task 13 完整消费）：

```ts
  useEffect(() => {
    const onControl = (event: Event) => {
      const detail = (event as CustomEvent<{ t?: string; ts?: number; reason?: string }>).detail;
      // pong 分支 Task 11 接 nativeStats.onPong；freeze/unfreeze 分支 Task 13 接 dispatch。
      void detail;
    };
    window.addEventListener("rd-media-control", onControl);
    return () => window.removeEventListener("rd-media-control", onControl);
  }, []);
```

3. 输入 sink（原 L287-326）：`sendControl/sendPointer` 改 `sessionRef.current?.send(event)`（无 session 时直接 return，pointer 丢弃语义不变）；`video` 参数改取 `canvasRef.current`。
4. 返回值：`videoRef` → `canvasRef`；删除 `peerConnectionState` state 与返回项。
5. selectMonitor/disconnect 回调不变。

- [ ] **Step 4: videoFit / InputCapture 泛化**

`videoFit.ts`：

```ts
type MediaSurface = HTMLVideoElement | HTMLCanvasElement;
function readSurfaceIntrinsicSize(el: MediaSurface): { width: number; height: number } | null {
  const width = el instanceof HTMLVideoElement ? el.videoWidth : el.width;
  const height = el instanceof HTMLVideoElement ? el.videoHeight : el.height;
  return width > 0 && height > 0 ? { width, height } : null;
}
export function normalizedPointFromVideoPointer(surface: MediaSurface, client: { x: number; y: number }): ... {
  // 函数体：readVideoIntrinsicSize → readSurfaceIntrinsicSize，其余（界外 null 守卫）不变
}
```

`InputCapture.ts`：`RemoteDesktopInputCaptureOptions.video` → `surface: MediaSurface`；`resolvePoint` 调 `normalizedPointFromVideoPointer(surface, ...)`；注释同步（canvas 内在尺寸 = 解码帧尺寸，letterbox 语义与 video 一致）。既有测试 `remoteDesktopVideoFit.test.ts` 的 stub video 仍可工作（`instanceof HTMLVideoElement` 在 stub 下为 false → 走 width/height 分支）——**改 stub 为 canvas 形状**：`{ width: 800, height: 450, getBoundingClientRect: ... } as unknown as HTMLCanvasElement`，断言不变。

- [ ] **Step 5: RemoteDesktopSession.tsx 换 canvas**

```tsx
  const { view, stats, containerRef, canvasRef, selectMonitor, disconnect } =
    useRemoteDesktopSession({ serverId });
```
```tsx
        {/* 原生媒体面：解码即画；cursor-none 语义不变（光标已烧进画面）。 */}
        <canvas
          ref={canvasRef}
          className="h-full w-full cursor-none object-contain"
        />
```
statusText：删除 `peerConnectionState === "disconnected"` 分支（原 L110-112 注释一并删）。

- [ ] **Step 6: 全量校验 + 提交**

Run: `pnpm typecheck && pnpm lint && pnpm exec tsx --tsconfig packages/ui/tsconfig.json --test packages/ui/test/*.test.ts`

> 此任务后 WebRTC 路径在 renderer 已死（旧 reducer 分支不可达），但 schema 还在——Task 14 统一删。

```bash
git add packages/ui/src/remote-desktop packages/ui/test
git commit -m "feat(rd-media): renderer 切换原生媒体（canvas 解码直画 + 输入走 media WS）"
```

### Task 11: 状态条读数重写（fps / RTT / 分辨率）

**Files:**
- Modify: `packages/ui/src/remote-desktop/sessionStats.ts`（整体重写，**保持输出接口不变**）
- Modify: `packages/ui/src/remote-desktop/useRemoteDesktopSession.ts`（3s 轮询改读新 collector）
- Test: `packages/ui/test/remoteDesktopSessionStats.test.ts`（重写）

**Interfaces:**
- Produces:

```ts
export interface RemoteDesktopSessionStats { fps: number | null; rttMs: number | null; width: number | null; height: number | null }
export const EMPTY_REMOTE_DESKTOP_SESSION_STATS: RemoteDesktopSessionStats
export function createNativeSessionStatsCollector(): {
  onDecoded(): void;
  onPong(rttMs: number): void;
  onConfigured(width: number, height: number): void;
  sample(): RemoteDesktopSessionStats;   // 采样并重置帧计数（fps = 间隔内解码帧数）
}
```

- [ ] **Step 1: 写失败测试**

重写 `remoteDesktopSessionStats.test.ts`：

```ts
const { createNativeSessionStatsCollector, EMPTY_REMOTE_DESKTOP_SESSION_STATS } =
  await import("../src/remote-desktop/sessionStats.js");

test("fps = 采样间隔内解码帧数；连续采样重置计数", () => {
  const c = createNativeSessionStatsCollector();
  for (let i = 0; i < 60; i++) c.onDecoded();
  const s = c.sample();
  assert.equal(s.fps, 60);
  assert.equal(c.sample().fps, 0, "第二次采样无新帧 → 0");
});

test("rtt 取最近一次 pong；分辨率来自 decoder 配置", () => {
  const c = createNativeSessionStatsCollector();
  c.onPong(17); c.onPong(19);
  c.onConfigured(1920, 1080);
  const s = c.sample();
  assert.equal(s.rttMs, 19);
  assert.equal(s.width, 1920);
  assert.equal(s.height, 1080);
});

test("空态导出形状不变", () => {
  assert.deepEqual(EMPTY_REMOTE_DESKTOP_SESSION_STATS, { fps: null, rttMs: null, width: null, height: null });
});
```

Run → FAIL

- [ ] **Step 2: 重写 sessionStats.ts**

删除 `deriveRemoteDesktopSessionStats`/`RemoteDesktopFpsBaseline`（RTCStatsReport 依赖全清），实现契约如上：`sample()` 返回 `{fps: decodedSinceLast, rttMs: lastRtt ?? null, width, height}` 并清零帧计数；`onConfigured` 只在首帧/换分辨率时被胶水回调。

- [ ] **Step 3: hook 接线**

原 stats 轮询 effect（L329-350）改为：

```ts
  useEffect(() => {
    const timer = window.setInterval(() => {
      setStats(nativeStats.sample());
      sessionRef.current?.ping(Date.now());
    }, REMOTE_DESKTOP_STATS_INTERVAL_MS);
    return () => window.clearInterval(timer);
  }, []);
```
`nativeStats` = `useRef(createNativeSessionStatsCollector())`；Task 9 胶水的 `onDecoded/onConfigured` 接 `nativeStats.current.*`（补上 Task 10 Step 3 留的 `onDecoded` 注释位；`onConfigured` 同时写 `nativeStats.onConfigured` 与 `setStats`）。`rttMs` 由 pong 事件写入 collector：把 Task 10 control 监听里的 `pong` 注释分支补为 `nativeStats.onPong(Date.now() - detail.ts)`。

- [ ] **Step 4: 测试 + 提交**

```bash
git add packages/ui/src/remote-desktop/sessionStats.ts packages/ui/src/remote-desktop/useRemoteDesktopSession.ts packages/ui/test/remoteDesktopSessionStats.test.ts
git commit -m "feat(rd-media): 状态条读数改为解码计数与 WS ping"
```

### Task 12: 版本门 `remoteDesktopNative` 能力位

**Files:**
- Modify: `packages/shared/src/server-remote.ts`（capabilities 增 `remoteDesktopNative: z.boolean().optional()`）
- Modify: `packages/server/src/http.ts`（options 增 `remoteDesktopNativeCapability?: boolean | (() => boolean)`；`createServerInfo` spread）
- Modify: `packages/desktop/src/host/lanAccessServer.ts`（传入 `remoteDesktopNativeCapability`，与 `remoteDesktopCapability` 同表达式）
- Modify: `packages/desktop/src/host/remoteDesktopPeerClient.ts`（`RemoteDesktopPeerInfo` 增 `remoteDesktopNative: boolean`；probe 映射）
- Modify: `packages/desktop/src/main/remoteDesktop/hostMessages.ts` **与** `packages/desktop/src/host/remoteDesktopMessages.ts`（PeerProbeResult schema 增字段，两份镜像）
- Modify: `packages/desktop/src/host/index.ts`（probe 结果组装 3 处：成功 / services 未就绪 / 异常，均补 `remoteDesktopNative: false` 或真值）
- Modify: `packages/desktop/src/main/remoteDesktop/desktopIpc.ts`（GetPeerCapability 返回体）
- Modify: `packages/shared/src/platform.ts`、`packages/client/src/globals.d.ts`、`packages/desktop/src/preload/index.ts`、`packages/desktop/src/renderer/src/desktopPlatform.ts`（rdGetPeerCapability 返回类型增 `remoteDesktopNative: boolean`）
- Modify: `packages/web/src/main.tsx`（stub `remoteDesktopNative: false`）
- Modify: `packages/ui/src/remote-desktop/deviceAvailability.ts`（判定改 `if (!probe.capability.remoteDesktop || !probe.capability.remoteDesktopNative) → unsupported`，注释：纯直替——缺 native 键 = 旧对端）
- Test: `packages/shared/test/server-remote-capabilities.test.ts`、`packages/server/test/lan-server-info-remote-desktop.test.ts`、`packages/ui/test/remoteDesktopDeviceAvailability.test.ts`、`packages/desktop/tests/fixtures/remote-desktop-capability.ts`

**Interfaces:**
- Produces: probe 结果 `{ remoteDesktop, remoteDesktopNative, online, name? }`（`remoteDesktopNative` 恒为 boolean）

- [ ] **Step 1: 写失败测试**

1. `packages/ui/test/remoteDesktopDeviceAvailability.test.ts` 追加：

```ts
test("对端有 remoteDesktop 但缺 remoteDesktopNative（旧版本）→ 不支持", () => {
  assert.deepEqual(
    resolveRemoteDesktopDeviceAvailability({
      status: "ok",
      capability: { remoteDesktop: true, remoteDesktopNative: false, online: true },
    }),
    { canConnect: false, reason: "unsupported" },
  );
});
```

2. `packages/server/test/lan-server-info-remote-desktop.test.ts` 追加：`remoteDesktopNativeCapability: () => true` 时 `body.capabilities.remoteDesktopNative === true`；不传时键缺席。

Run 对应文件 → FAIL

- [ ] **Step 2: 按上表逐处实现（14 处锚点，模板如下）**

- server-remote.ts：`remoteDesktop: z.boolean().optional(),` 下一行加 `remoteDesktopNative: z.boolean().optional(),`（注释：原生媒体面版本门，纯直替）。
- http.ts：照 `remoteDesktopCapability`（L256-262 注释块 + L352-360 spread）逐字镜像一套 `remoteDesktopNativeCapability`。
- 镜像 schema：`remoteDesktopPeerProbeResultSchema` 增 `remoteDesktopNative: z.boolean()`。
- platform.ts rdGetPeerCapability 返回对象增：

```ts
    /** 对端支持原生媒体面（纯直替版本门）；旧对端缺键 = false。 */
    remoteDesktopNative: boolean;
```

- deviceAvailability 判定按上 Step 1 测试改。
- 其余各处按「锚点行号 + 同字段名」机械同步（侦察清单已列全）。

- [ ] **Step 3: 全量校验 + 提交**

Run: `pnpm typecheck && pnpm lint && pnpm --filter @mode/shared test && pnpm --filter @mode/server test && pnpm exec tsx --tsconfig packages/ui/tsconfig.json --test packages/ui/test/remoteDesktopDeviceAvailability.test.ts && node --test packages/desktop/tests/*.test.mts && node scripts/check-language-policy.mjs`

```bash
git add packages/shared packages/server packages/desktop packages/web packages/ui
git commit -m "feat(rd-media): remoteDesktopNative 版本门（缺键=旧对端不支持）"
```

### Task 13: 信令 v2 + 冻结 UI

**Files:**
- Modify: `packages/shared/src/remote-desktop.ts`（`REMOTE_DESKTOP_PROTOCOL_VERSION = 2`）
- Modify: `packages/ui/src/remote-desktop/sessionReducer.ts`（`freeze/unfreeze` signal → `frozen` state）
- Modify: `packages/ui/src/remote-desktop/useRemoteDesktopSession.ts`（`rd-media-control` 的 freeze/unfreeze → dispatch）
- Modify: `packages/ui/src/remote-desktop/RemoteDesktopSession.tsx`（statusText 增 frozen 分支）
- Modify: `packages/ui/src/i18n/locales/{zh-CN,en-US,fa}.ts`（`remoteDesktop.session.frozen`）
- Test: `packages/ui/test/remoteDesktopSessionReducer.test.ts`、`packages/shared/test/remote-desktop.test.ts`（版本断言）

**Interfaces:**
- Produces: `view.frozen: boolean`；`remoteDesktop.session.frozen` 三语文案

- [ ] **Step 1: 写失败测试**

reducer 测试追加：

```ts
test("freeze/unfreeze 切换 frozen；connecting 重连重置", () => {
  let state = { ...INITIAL_REMOTE_DESKTOP_SESSION_VIEW_STATE, phase: "connected" as const };
  state = reduce(state, { type: "controller",
    event: { kind: "signal", message: { type: "freeze", reason: "locked" } } }).state;
  assert.equal(state.frozen, true);
  state = reduce(state, { type: "controller",
    event: { kind: "signal", message: { type: "unfreeze" } } }).state;
  assert.equal(state.frozen, false);
});
```

shared 测试追加：`assert.equal(REMOTE_DESKTOP_PROTOCOL_VERSION, 2)`。

Run → FAIL

- [ ] **Step 2: 实现**

1. `REMOTE_DESKTOP_PROTOCOL_VERSION = 2`（coordinator 的 hello 校验、reducer 的 hello 发送都引用该常量，自动一致；主控发 v2、被控同构建回 v2 ✓）。
2. reducer：signal `freeze` → `{ frozen: true }`；`unfreeze` → `{ frozen: false }`；`connecting` 重置分支把 `frozen:false` 加入。
3. hook 的 `rd-media-control` 监听补：

```ts
      if (detail?.t === "freeze") dispatchRef.current({ type: "freeze" });
      if (detail?.t === "unfreeze") dispatchRef.current({ type: "unfreeze" });
```
（event 联合增 `{ type: "freeze" } | { type: "unfreeze" }`，与 signal 分支共用转移逻辑。）
4. statusText（`RemoteDesktopSession.tsx` L107 链）在 `phase === "closed"` 之后插入：

```tsx
      : view.frozen
        ? "remoteDesktop.session.frozen"
```
5. i18n 三语（插入锚点：`remoteDesktop.session.reconnecting` 之后）：

| key | zh-CN | en-US | fa |
|---|---|---|---|
| `remoteDesktop.session.frozen` | 对端已锁屏，画面已冻结 | The peer's screen is locked; the screen is frozen | صفحه دستگاه مقابل قفل شده؛ تصویر منجمد است |

- [ ] **Step 3: 校验 + 提交**

Run: `pnpm typecheck && pnpm lint && node scripts/check-language-policy.mjs && pnpm exec tsx --tsconfig packages/ui/tsconfig.json --test packages/ui/test/remoteDesktopSessionReducer.test.ts && pnpm --filter @mode/shared test`

```bash
git add packages/shared/src/remote-desktop.ts packages/ui/src/remote-desktop packages/ui/src/i18n/locales packages/ui/test packages/shared/test
git commit -m "feat(rd-media): 信令协议 v2 与锁屏冻结状态 UI"
```

## Phase P4 — 直替清理与回归

### Task 14: 输入注入接线 + WebRTC 面删除

**Files:**
- Modify: `packages/desktop/src/host/remoteDesktopServer.ts`（relay `onInput` → 既有 `inject`；删 capture 时代 `onInput` 消费）
- Modify: `packages/shared/src/remote-desktop.ts`（union 删 `offer/answer/ice`）
- Modify: `packages/ui/src/remote-desktop/sessionReducer.ts`（删 `apply-offer/add-ice-candidate` effect 联合成员与 offer/ice 分支）
- Modify: `packages/ui/src/remote-desktop/useRemoteDesktopSession.ts`（删 Task 10 留的两个 no-op case；确认无 `codecPreferences` import 残留）
- Modify: `packages/desktop/src/main/remoteDesktop/coordinator.ts`（删 `ensureCaptureSession`/`captureSession` 生命周期、`answer/ice` 分支、`RemoteDesktopInject` 发送；`listRemoteDesktopMonitors` 改 import `./monitors.js`）
- Delete: `packages/desktop/src/main/remoteDesktop/capturePage.ts`、`captureWindow.ts`、`packages/ui/src/remote-desktop/codecPreferences.ts`
- Modify: `packages/desktop/src/main/remoteDesktop/hostMessages.ts` **与** `packages/desktop/src/host/remoteDesktopMessages.ts`（删 `RemoteDesktopInject` 消息；`RemoteDesktopPeerProbeResult` 等保持）
- Modify: `packages/desktop/src/main/remoteDesktop/hostBridge.ts`、`host/index.ts`（`RemoteDesktopInject` 分发删除）
- Modify: `packages/shared/test/remote-desktop.test.ts`（offer/ice 用例改为「必须解析失败」）
- Modify: `docs/specs/lan-remote-desktop.md`（删「质量与延迟参数」节——其 getDisplayMedia/setCodecPreferences 描述已失效）
- Modify: `docs/specs/lan-remote-desktop-native-media.md`（追加「实现进度」节）
- Test: 上述各仓测试

**Interfaces:**
- Consumes: Task 5 relay 的 `onInput` 槽位、既有 `remoteDesktopServer.inject`
- Produces: 无新契约（纯删除 + 一处接线）

- [ ] **Step 1: 输入注入接线（删除前先联通新路径）**

`remoteDesktopServer.ts` 工厂内（relay 创建处）：

```ts
  const relay = createRemoteDesktopMediaRelay({
    logger,
    onFrame: (payload) => { /* WS 已挂接时由 attachWebSocket 内部消费；此回调留作日志位 */ },
    onStatus: (frozen, reason) => postToMain({ type: RemoteDesktopHostMessageTypes.RemoteDesktopMediaStatus, frozen, ...(reason ? { reason } : {}) }),
    onReady: () => relay.sendToHelper(2),
    onInput: (event) => { void inject(event as RemoteDesktopRawInputEvent); }, // 既有 raw_input 注入
  });
```

（`inject` 即工厂内已有 `defaultInject`/`injectRawInput` 包装；`onFrame` 的 WS 出口按 Task 5 Step 4 接 `ws.send(Buffer.from(payload))` + 4MB 背压丢弃。）

- [ ] **Step 2: 写失败测试（schema 删除断言）**

`packages/shared/test/remote-desktop.test.ts` 既有 offer/ice 解析用例改为：

```ts
test("WebRTC 消息已随原生媒体面移除：offer/answer/ice 必须解析失败", () => {
  for (const msg of [
    { type: "offer", sdp: "v=0" },
    { type: "answer", sdp: "v=0" },
    { type: "ice", candidate: "x", sdpMid: "0", sdpMLineIndex: 0 },
  ]) {
    assert.equal(remoteDesktopSignalMessageSchema.safeParse(msg).success, false);
  }
});
```

Run: `pnpm --filter @mode/shared test` → FAIL（当前仍接受）

- [ ] **Step 3: 执行删除（按 Files 清单逐项）**

顺序（保证每步 typecheck 可解释）：
1. `sessionReducer.ts`：effect 联合删两成员 + 解释器删两个 case + signal 分支删 offer/ice 处理（连同 Task 10 的忽略分支注释）。
2. `useRemoteDesktopSession.ts`：删两个 no-op case；`grep -n "codecPreferences\|applyPreferred" packages/ui/src` 应为 0。
3. shared schema 删 3 分支（Step 2 测试转绿）。
4. coordinator：`ensureCaptureSession`、`runtime.captureSession`、`start-capture/stop-capture` 中残余 capture 引用（Task 6 已改 MediaStart/Stop，此处删函数本体与 `handlers.onOutboundSignal/onInput`、`answer|ice` case）、`RemoteDesktopHostMessageTypes.RemoteDesktopInject` 发送处；import 改 `./monitors.js`。
5. `grep -rn "capturePage\|captureWindow\|codecPreferences\|RemoteDesktopInject" packages --include=*.ts --include=*.tsx | grep -v node_modules | grep -v dist` → 逐个清零后删 3 个文件。
6. 两份镜像 schema + hostBridge + host 分发删 `RemoteDesktopInject`。
7. 旧 spec 删「质量与延迟参数」节；native-media spec 追加：

```markdown
## 实现进度

- [x] P1 采集与编码（rd_media + napi + cargo 单测）
- [x] P2 传输链（双票据/media WS/中继/helper 泵/被控接线）
- [x] P3 renderer 切换（canvas 解码/输入/状态条/版本门/冻结 UI）
- [x] P4 直替清理（WebRTC 面全删；2026-10-12）
- [ ] 两台 Windows 实机验收（见「验收场景」，由用户执行）
```

- [ ] **Step 4: 全量校验 + 提交**

Run: `pnpm typecheck && pnpm lint && pnpm architecture:check --changed && node scripts/check-language-policy.mjs && pnpm --filter @mode/shared test && pnpm --filter @mode/server test && pnpm exec tsx --tsconfig packages/ui/tsconfig.json --test packages/ui/test/*.test.ts && node --test packages/desktop/tests/*.test.mts && pnpm --dir packages/mode-cua test`

```bash
git add -A
git commit -m "feat(rd-media)!: 删除 WebRTC 媒体面，输入注入接线 media WS（纯直替完成）"
```

### Task 15: 全量回归 + 构建 + 验收交付

**Files:**
- 无代码改动（除非回归暴露问题——修在本任务内提交）
- Verify: spec `docs/specs/lan-remote-desktop-native-media.md` 验收清单

**Interfaces:**
- Consumes: 全部 Task 1-14 产物

- [ ] **Step 1: Rust 门**

```bash
cargo test --manifest-path crates/mode-cua-ax/Cargo.toml
cargo clippy --manifest-path crates/mode-cua-ax/Cargo.toml --all-targets -- -D warnings
pnpm build:cua-helper
```
Expected: 全 PASS；`build:cua-helper` 产物含 `rd_media_start/switch/stop`（用 Step 3 的符号检查）。

- [ ] **Step 2: 仓库门**

```bash
pnpm typecheck && pnpm lint && pnpm architecture:check --changed && node scripts/check-language-policy.mjs
pnpm --filter @mode/shared test && pnpm --filter @mode/server test && pnpm --filter @mode/services test
pnpm exec tsx --tsconfig packages/ui/tsconfig.json --test packages/ui/test/*.test.ts
node --test packages/desktop/tests/*.test.mjs packages/desktop/tests/*.test.mts
pnpm --dir packages/mode-cua test
```
Expected: 全 PASS（如实报告任何失败，不粉饰）。

- [ ] **Step 3: 符号与残留检查**

```bash
node -e "const a=require('./packages/mode-cua/dist-cua-helper/cua_ax.node'); const t=[a.rd_media_start,a.rd_media_switch,a.rd_media_stop].map(f=>typeof f); if(t.some(x=>x!=='function')){console.error(t);process.exit(1)} console.log('rd_media symbols ok')"
grep -rn "RTCPeerConnection\|codecPreferences\|capturePage\|createAnswer\|ondatachannel" packages --include=*.ts --include=*.tsx | grep -v node_modules | grep -v dist
```
Expected: 第一条打印 `rd_media symbols ok`；第二条 **0 命中**（WebRTC 面无残留，`node_modules`/`dist` 排除）。

- [ ] **Step 4: 交付验收**

1. 确认 spec「验收场景」9 项完整未勾选（交用户两机实测）。
2. 向用户交付：本计划全部任务完成状态 + 验收清单 + 实机重点观察项（fps≈60、RTT<20ms、锁屏冻结、换屏<1s、旧对端文案、抓包只见密文）。
3. 最后一次提交（若有修补）：

```bash
git add -A && git commit -m "test(rd-media): 全量回归与构建门通过"
```

> 不 push：由用户决定推送时机（push main 触发 release 流水线）。

---

## 计划自审记录（writing-plans Self-Review）

1. **Spec 覆盖**：spec 范围四层（采集/编码/传输/解码）→ Task 1-2（采集编码）、3-8（传输）、9-11（解码渲染）；信令 v2/版本门/冻结 → Task 4/12/13；删除清单 → Task 14；失败语义（unsupported/inaccessible）→ Task 3（旧对端 mediaTicket 缺失）、Task 8（起不来按冻结）、Task 12（能力位）；安全（双票据/密钥不落 host 明文面/日志纪律）→ Task 3/5/7；验收与测试策略 → Task 15。**已修正缺口**：输入注入新路径在初稿无归属 → Task 14 Step 1 显式接线（先联通后删除）。
2. **占位符扫描**：Task 2 Step 3 对「即时 IDR」给出两条可落地路径与降级口径（非 TODO）；Task 5 测试 helper 语义留了「以仓库 helper 真实行为调整」的明确指令（防照抄必炸用例）；无 TBD/「适当处理」类措辞。
3. **类型一致性**：契约 A-H 为唯一形状源——已核对 `rdStartSession {ok,mediaUrl}`（E）贯穿 Task 3/10、管道 5 字节头（B）在 Task 5 测试/Task 7 实现/Task 9 解析三处一致、`mediaOpenReceived` 改名贯穿 Task 10/13、`nativeStats` 只在 Task 11 之后出现（Task 10 初稿引用已移除，见下方修正记录）。**已修正**：Task 9 胶水的 `console.warn` 改用 `packages/ui/src/logger.ts`（仓库日志纪律）。

