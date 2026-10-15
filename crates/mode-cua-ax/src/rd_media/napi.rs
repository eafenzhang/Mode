//! 契约 A 的 napi 面（`rd_media_start / rd_media_switch / rd_media_stop`）与采集线程。
//!
//! 形状以计划「关键跨任务契约 A」为唯一权威，Task 7 按此写 JS：
//! - `onFrame/onStatus` 是**单参** JS 回调（`(frame) => void` / `(s) => void`）——
//!   故 TSFN 用 `ErrorStrategy::Fatal`（默认 CalleeHandled 会把 `(null, value)`
//!   塞给 JS，首参变 null，直接破坏 Task 7 的消费代码）；brief 代码块里
//!   `ThreadsafeFunction<T>` 未写第二泛型参数，默认策略与契约 A 的 TS 形状矛盾，
//!   此处以契约 A 的 TS 形状为准。
//! - 采集跑在专用 `std::thread`（简报 Step 4）；`start` 抢占式先 `stop`。
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use napi::bindgen_prelude::Buffer;
use napi::threadsafe_function::{ErrorStrategy, ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_derive::napi;

use super::capture::{CaptureFrame, ComScope, DuplicationOutput};
use super::encode::{Encoder, RdEncodeCodec};
use super::warn;
use crate::error::{AxError, AxResult};

#[napi(object)]
pub struct RdMediaRect {
  pub x: i32,
  pub y: i32,
  pub width: u32,
  pub height: u32,
}

/// 帧回调载荷：seq 单调（wire 的 IV = 会话盐∥seq，安全契约）、key = 码流含 IDR、
/// captured_at_ms = 取帧时刻打点（Unix 毫秒 u32 截断，随帧线上传递供主控算帧龄）、
/// data = Annex-B ES（helper 加密后进管道 9 字节头：seq4+flags1+ts4）。
#[napi(object)]
pub struct RdMediaFrameNapi {
  pub seq: u32,
  pub key: bool,
  pub captured_at_ms: u32,
  pub data: Buffer,
}

/// 状态回调载荷（use_nullable 按简报：frozen 恒在，取不到 reason 落 null）。
#[napi(object, use_nullable = true)]
pub struct RdMediaStatusNapi {
  pub frozen: bool,
  pub reason: Option<String>,
}

// 入参对象：只需要 FromNapiValue（JS → Rust）。关掉 ToNapiValue 生成——TSFN 字段
// 没有 ToNapiValue 实现，生成方向会在编译期报缺 trait（object_to_js = false 是
// napi-derive 提供的官方开关，d.ts 类型生成不受影响）。
#[napi(object, object_to_js = false)]
pub struct RdMediaStartOptions {
  /// 物理像素矩形（VIRTUALDESK 口径）。
  pub rect: RdMediaRect,
  /// 编码输出宽上限（高度按 rect 比例推导），典型 1920。
  pub max_width: u32,
  pub fps: u32,
  pub bitrate_bps: u32,
  /// 编码 codec 偏好（spec: native-media「HEVC 选型」2026-10-13）：缺省/"h264" = H.264；
  /// "hevc" 无硬件编码器时回退 H.264（warn，不失败——解码侧按码流自描述跟随）；
  /// 其它值按 invalid_request 拒绝（JS 契约层已拦，这里是防御）。
  pub codec: Option<String>,
  pub on_frame: ThreadsafeFunction<RdMediaFrameNapi, ErrorStrategy::Fatal>,
  pub on_status: ThreadsafeFunction<RdMediaStatusNapi, ErrorStrategy::Fatal>,
}

/// 编码输出尺寸：不放大、保持比例、双向取偶（H.264 要求偶数尺寸）。
/// 纯函数，tests/rd_media.rs 覆盖。
pub fn compute_out_size(width: u32, height: u32, max_width: u32) -> (u32, u32) {
  let (width, height) = (width.max(1), height.max(1));
  let out_w = (width.min(max_width.max(1)) & !1).max(2);
  let scaled = ((height as u64) * (out_w as u64) / (width as u64)) as u32;
  (out_w, (scaled & !1).max(2))
}

/// switch 待消费矩形槽（worker 单消费者）。
type PendingRect = Arc<Mutex<Option<(i32, i32, i32, i32)>>>;

/// worker 句柄：身份 + stop 标志 + switch 待消费矩形 + join 句柄。
struct MediaWorkerHandle {
  /// worker 身份（全局自增）：worker 退出时按身份清自己的句柄——不误删后来者。
  worker_id: u64,
  stop: Arc<AtomicBool>,
  pending_rect: PendingRect,
  join: Option<JoinHandle<()>>,
}

/// 全局唯一会话槽（简报：`static STARTED: Mutex<Option<MediaWorkerHandle>>`）。
static STARTED: Mutex<Option<MediaWorkerHandle>> = Mutex::new(None);
/// worker 身份发号器（见 `MediaWorkerHandle::worker_id`）。
static NEXT_WORKER_ID: AtomicU64 = AtomicU64::new(1);
/// start 等 worker 完成建流（DUPL + 编码器）的超时：正常 <1s，超时视为卡死。
const SETUP_TIMEOUT: Duration = Duration::from_secs(10);

/// 锁静态量：锁中毒取回守卫继续（临界区只有短时 Option/矩形读写，无毒源；
/// deny(unwrap_used) 下用 unwrap_or_else 而非 unwrap）。
fn locked<T>(m: &Mutex<T>) -> MutexGuard<'_, T> { m.lock().unwrap_or_else(|e| e.into_inner()) }

#[napi(js_name = "rd_media_start")]
pub fn rd_media_start_napi(opts: RdMediaStartOptions) -> napi::Result<()> {
  crate::to_napi(start(opts))
}

#[napi(js_name = "rd_media_switch")]
pub fn rd_media_switch_napi(rect: RdMediaRect) -> napi::Result<()> {
  crate::to_napi(switch(rect))
}

#[napi(js_name = "rd_media_stop")]
pub fn rd_media_stop_napi() -> napi::Result<()> {
  stop();
  Ok(())
}

/// 紧急关键帧请求：renderer 侧解码落后/丢帧后经 WS → relay → helper 调到这里；
/// worker 下一 tick 消费并 `force_idr`（下一帧起产出新 IDR，不丢帧、不断流——
/// 语义与 switch/解冻的强制关键帧一致）。一次性标志：不允许积压。
static PENDING_KEY: AtomicBool = AtomicBool::new(false);

#[napi(js_name = "rd_media_request_key")]
pub fn rd_media_request_key_napi() {
  PENDING_KEY.store(true, Ordering::Relaxed);
}

/// 动态码率（自适应链路经同一控制面调到这里）：worker 下一 tick 消费；
/// 0 = 无请求；编码器不支持（ICodecAPI 缺失/失败）时静默保持原码率，不断流。
static PENDING_BITRATE: AtomicU32 = AtomicU32::new(0);

#[napi(js_name = "rd_media_set_bitrate")]
pub fn rd_media_set_bitrate_napi(bitrate_bps: u32) {
  PENDING_BITRATE.store(bitrate_bps, Ordering::Relaxed);
}

/// 测试专用：无 start 时 stop 幂等（不触 DUPL/MF，无桌面会话也能跑）。
pub fn stop_for_tests() { stop(); }

/// 启动：抢占式先收旧会话 → 起采集线程，**建流（DUPL + 编码器）在 worker 内
/// 同步完成并经 channel 回传结果**——失败原样转成 JS 错误（不藏进线程），成功
/// 才落 STARTED 句柄。
///
/// 为什么建流挪进 worker：① MFTEnumEx/ActivateObject/DuplicateOutput 需要调用
/// 线程已初始化 COM，而 JS（Node helper）线程未必——worker 内用 `ComScope`
/// （capture.rs 的 RAII 守卫）自管 COM，JS 线程零 Win32；② Win32 调用保持全部
/// 圈在 capture/encode 模块（组织约束）。
fn start(opts: RdMediaStartOptions) -> AxResult<()> {
  let rect = (
    opts.rect.x,
    opts.rect.y,
    opts.rect.width as i32,
    opts.rect.height as i32,
  );
  if rect.2 <= 0 || rect.3 <= 0 {
    return Err(AxError::new("invalid_request", "采集矩形宽高必须为正"));
  }
  if opts.fps == 0 || opts.max_width == 0 || opts.bitrate_bps == 0 {
    return Err(AxError::new("invalid_request", "fps/maxWidth/bitrateBps 必须为正"));
  }
  let codec = match opts.codec.as_deref() {
    None | Some("h264") => RdEncodeCodec::H264,
    Some("hevc") => RdEncodeCodec::Hevc,
    Some(other) => {
      return Err(AxError::new("invalid_request", format!("不支持的 codec: {other}")));
    }
  };
  stop(); // 已在跑先 stop 再起新线程（简报 Step 4）
  let stop_flag = Arc::new(AtomicBool::new(false));
  let pending_rect = Arc::new(Mutex::new(None));
  let worker_id = NEXT_WORKER_ID.fetch_add(1, Ordering::Relaxed);
  let (ready_tx, ready_rx) = mpsc::channel::<AxResult<()>>();
  // 两段握手的第二段：start 把句柄落进全局槽后才放行 worker 进循环——否则
  // worker 建流成功后立刻出错退出、在落槽前 clear_self 会留假活句柄（竞态）。
  let (go_tx, go_rx) = mpsc::channel::<()>();
  let spawn = {
    let stop_f = Arc::clone(&stop_flag);
    let pending = Arc::clone(&pending_rect);
    let on_frame = opts.on_frame;
    let on_status = opts.on_status;
    let max_width = opts.max_width;
    let fps = opts.fps;
    let bitrate = opts.bitrate_bps;
    thread::Builder::new()
      .name("rd-media-capture".into())
      .spawn(move || {
        worker_loop(
          rect, max_width, fps, bitrate, codec, stop_f, pending, on_frame, on_status, worker_id,
          ready_tx, go_rx,
        )
      })
  };
  let join = spawn.map_err(|e| AxError::internal(format!("采集线程创建失败: {e}")))?;
  match ready_rx.recv_timeout(SETUP_TIMEOUT) {
    // 建流成功：先把句柄交给全局槽（switch 从这里起可见），再放行 worker 进循环。
    Ok(Ok(())) => {
      *locked(&STARTED) =
        Some(MediaWorkerHandle { worker_id, stop: stop_flag, pending_rect, join: Some(join) });
      let _ = go_tx.send(());
      Ok(())
    }
    // 建流失败：worker 已退出（dup/enc 随其作用域释放），错误直回 JS；句柄不落槽。
    // 不内联 join——即使驱动在 RAII 释放里卡住也不阻塞 JS 线程，转后台收尸。
    Ok(Err(e)) => {
      let _ = thread::Builder::new().name("rd-media-reap".into()).spawn(move || {
        let _ = join.join();
      });
      Err(e)
    }
    // 超时（驱动卡死）：置 stop 放弃该 worker，转后台收尸，回 timeout 类错误。
    Err(mpsc::RecvTimeoutError::Timeout) => {
      stop_flag.store(true, Ordering::Relaxed);
      drop(go_tx); // worker 若在等放行，收到断开即退出
      let _ = thread::Builder::new().name("rd-media-reap".into()).spawn(move || {
        let _ = join.join();
      });
      Err(AxError::internal("采集初始化超时（10s），已放弃该 worker"))
    }
    // 通道断开 = worker 在建流中提前终止（panic/被杀）——同样不留句柄，快速报错。
    Err(mpsc::RecvTimeoutError::Disconnected) => {
      drop(go_tx);
      let _ = thread::Builder::new().name("rd-media-reap".into()).spawn(move || {
        let _ = join.join();
      });
      Err(AxError::internal("采集线程在建流期间提前终止"))
    }
  }
}

/// 换屏：只写 pending 矩形，由 worker 循环消费（重开 duplication + 强制 IDR；
/// 输出尺寸变化时重建编码器并迁移状态机——seq 跨重建连续是 IV 唯一性安全契约）。
fn switch(rect: RdMediaRect) -> AxResult<()> {
  let r = (rect.x, rect.y, rect.width as i32, rect.height as i32);
  if r.2 <= 0 || r.3 <= 0 {
    return Err(AxError::new("invalid_request", "采集矩形宽高必须为正"));
  }
  let started = locked(&STARTED);
  let handle = started.as_ref().ok_or_else(|| {
    // worker 错误退出时已按身份清过句柄（clear_self），这里如实报「未启动」——
    // 对死 worker 幂等且可观测（JS 收到 invalid_request，而不是写进死队列）。
    warn("rd_media_switch 被拒：无活动采集会话（未启动或 worker 已退出）");
    AxError::new("invalid_request", "rd_media 未启动（switch 需在 start 之后）")
  })?;
  *locked(&handle.pending_rect) = Some(r);
  Ok(())
}

/// 停止：置 flag + join（幂等；无 start 直接返回）。join 超 2s 视为 worker 卡死，
/// 放弃等待（detach 语义）——worker 最多再持有 16ms 量级的采集 tick，进程内资源
/// 由其自然退出释放；日志记 warn（简报 Step 4）。
fn stop() {
  let handle = locked(&STARTED).take();
  let Some(mut h) = handle else { return };
  h.stop.store(true, Ordering::Relaxed);
  let Some(jh) = h.join.take() else { return };
  // join 不能带超时：转手给临时线程执行，主线程按 2s 收结果。
  let (tx, rx) = mpsc::channel::<()>();
  let spawn = thread::Builder::new().name("rd-media-stop".into()).spawn(move || {
    let _ = jh.join();
    let _ = tx.send(());
  });
  match spawn {
    Ok(_) => {
      if rx.recv_timeout(Duration::from_secs(2)).is_err() {
        warn("rd_media_stop join 超时（2s），worker 线程转为 detach 继续自行退出");
      }
    }
    Err(_) => warn("rd_media_stop 无法创建 join 线程，worker 转为 detach"),
  }
}

/// 采集主循环（专用线程）。冻结表达：`dup = None`（duplication 已释放）+ 状态机
/// frozen；恢复靠「冻结期节流重开 duplication」——DXGI 的 ACCESS_LOST 必须重建
/// duplication 才能恢复（简报伪码把恢复挂在 NewDesktop 分支是不可达路径：失效的
/// duplication 不会再 acquire 到新帧），故此处改为关闭后轮询重开。
#[allow(clippy::too_many_arguments)]
fn worker_loop(
  mut rect: (i32, i32, i32, i32),
  max_width: u32,
  fps: u32,
  bitrate: u32,
  codec: RdEncodeCodec,
  stop: Arc<AtomicBool>,
  pending_rect: PendingRect,
  on_frame: ThreadsafeFunction<RdMediaFrameNapi, ErrorStrategy::Fatal>,
  on_status: ThreadsafeFunction<RdMediaStatusNapi, ErrorStrategy::Fatal>,
  worker_id: u64,
  ready: mpsc::Sender<AxResult<()>>,
  go: mpsc::Receiver<()>,
) {
  // COM（MTA，RAII）：MF/部分硬件 MFT 假定调用线程已初始化 COM——建流与整个
  // 采集循环都在本线程，作用域结束（含错误提前返回）自动配对 CoUninitialize。
  let _com = ComScope::enter();
  // 建流在 worker 内同步做（start 经 channel 等结果）：失败把 AxError 回传给
  // start 直转 JS，本线程随即退出（句柄尚未落 STARTED，无假活风险）。
  let dup = match DuplicationOutput::open(rect) {
    Ok(d) => d,
    Err(e) => {
      let _ = ready.send(Err(e));
      return;
    }
  };
  let (out_w, out_h) = compute_out_size(rect.2 as u32, rect.3 as u32, max_width);
  let mut enc = match Encoder::new(dup.device(), out_w, out_h, fps, bitrate, codec) {
    Ok(e) => e,
    Err(e) => {
      let _ = ready.send(Err(e)); // dup 随本作用域释放
      return;
    }
  };
  let mut dup_slot: Option<DuplicationOutput> = Some(dup);
  if ready.send(Ok(())).is_err() {
    return; // start 已超时放弃（stop_flag 亦已置位）——不开流直接退出
  }
  // 等 start 把句柄落进全局槽后再进循环（两段握手，见 start 内注释）：
  // 通道断开 = start 失败/超时放弃，直接退出。
  if go.recv().is_err() {
    return;
  }
  let mut frozen = false;
  while !stop.load(Ordering::Relaxed) {
    // 紧急关键帧请求：一次性消费（丢帧恢复 / renderer 请求都不允许积压）；
    // 冻结期也照常置位——解冻路径本就 force_idr，置位只可能有冗余、不会有害。
    let key_requested = PENDING_KEY.swap(false, Ordering::Relaxed);
    if key_requested {
      enc.force_idr();
    }
    // 动态码率：一次性消费（自适应链路按采样周期下发，不允许积压）。
    let want_bps = PENDING_BITRATE.swap(0, Ordering::Relaxed);
    if want_bps > 0 && !enc.set_bitrate(want_bps) {
      warn(&format!("动态码率 {want_bps} 未生效（编码器不支持或拒绝），保持原码率"));
    }
    // ① 消费换屏请求。
    if let Some(new_rect) = locked(&pending_rect).take() {
      if new_rect != rect {
        match reopen_target(&mut dup_slot, &mut enc, &mut rect, new_rect, max_width, fps, bitrate)
        {
          Ok(()) => {}
          // 新屏打不开：保持旧屏继续流（会话可恢复性优先于立即失败）。
          Err(e) => warn(&format!("换屏重开失败，维持原输出: {e}")),
        }
      }
    }
    // ② 冻结态：节流重开 duplication，成功即解冻（状态机 set_frozen(false)
    //    自带 force_idr → 解冻后首帧 IDR，spec「恢复 → unfreeze + 强制 IDR」）。
    let Some(d) = dup_slot.as_mut() else {
      match DuplicationOutput::open(rect) {
        Ok(nd) => {
          dup_slot = Some(nd);
          frozen = false;
          enc.set_frozen(false);
          send_status(&on_status, false, None);
        }
        Err(_) => thread::sleep(Duration::from_millis(50)), // 仍不可用（锁屏中）
      }
      continue;
    };
    // ③ 取帧。
    let frame = d.next_frame(16);
    let mut event = match frame {
      Ok(f) => f,
      Err(e) => {
        // 一次性自愈：重建采集链（驱动复位/设备移除/会话切换等瞬时错误的主场）。
        // 重建成功继续出流；失败才上报 frozen 并退出（对端黑屏前必有可观测信号，
        // 契约 A reason 为 optional string，用可区分的错误归因串）。
        warn(&format!("采集错误，尝试重建采集链: {e}"));
        match rebuild_chain(&mut dup_slot, &mut enc, rect, max_width, fps, bitrate) {
          Ok(()) => continue,
          Err(e2) => {
            warn(&format!("采集链重建失败，worker 退出: {e2}"));
            send_status(&on_status, true, Some("capture-error"));
            break;
          }
        }
      }
    };
    // ③b 键请求补帧（2026-10-14 实机根因）：桌面与光标都静止时 `next_frame` 只会给
    //     Idle（正常环境走不到 user32 补帧路径），上面的 `force_idr` 便永远没有帧可编，
    //     对端表现为「画面已连接 / 0 fps / 0.0 Mbps」的纯黑。这里把最后一帧重发一次，
    //     让本 tick 的 IDR 立刻有内容兑现——「开流」与「丢帧恢复」两种键请求都走这条。
    if key_requested && matches!(event, CaptureFrame::Idle) {
      match d.reemit() {
        Ok(resent) => event = resent,
        // 补帧失败不升级为会话级失败：维持 Idle，等下一 tick / 下一次真实变化。
        Err(e) => warn(&format!("键请求补帧失败，维持 Idle: {e}")),
      }
    }
    match event {
      CaptureFrame::Idle => {}
      CaptureFrame::AccessLost => {
        if !frozen {
          frozen = true;
          enc.set_frozen(true);
          send_status(&on_status, true, Some("access-lost"));
        }
        // 释放 duplication（简报 close 语义），转入 ② 的轮询重开。
        dup_slot = None;
      }
      CaptureFrame::NewDesktop { texture, .. } => {
        if frozen {
          // 防御分支（简报伪码同款）：理论不可达（解冻只发生在 ②），保底同步状态。
          frozen = false;
          enc.set_frozen(false);
          send_status(&on_status, false, None);
        }
        // 取帧时刻打点：帧龄（glass-to-glass 近似）的起点，随帧一路传到主控绘制处。
        let captured_at_ms = unix_ms_u32();
        match enc.push(&texture, captured_at_ms) {
          Ok(Some(chunk)) => {
            let payload = RdMediaFrameNapi {
              seq: chunk.seq,
              key: chunk.key,
              captured_at_ms: chunk.captured_at_ms,
              data: Buffer::from(chunk.data),
            };
            // NonBlocking（简报）：JS 消费慢只压 TSFN 队列，不反压采集线程。
            let _ = on_frame.call(payload, ThreadsafeFunctionCallMode::NonBlocking);
          }
          Ok(None) => {}
          Err(e) => {
            // 一次性自愈：重建编码器（迁移状态机保 seq 连续）；采集侧保留不动。
            // 失败才上报 frozen 并退出（同上：可观测信号先于退出）。
            warn(&format!("编码错误，尝试重建编码器: {e}"));
            let rebuilt = dup_slot
              .as_ref()
              .ok_or_else(|| AxError::internal("重建编码器时 duplication 缺失"))
              .and_then(|d| rebuild_encoder(d, &mut enc, rect, max_width, fps, bitrate));
            match rebuilt {
              Ok(()) => continue,
              Err(e2) => {
                warn(&format!("编码器重建失败，worker 退出: {e2}"));
                send_status(&on_status, true, Some("encode-error"));
                break;
              }
            }
          }
        }
      }
    }
  }
  // 收口：dup/enc 在作用域尾随 drop（DUPL 释放 + MF drain/Shutdown 配对）；
  // 按身份清掉自己的全局句柄——不留假活句柄，之后的 switch/stop 对死 worker
  // 幂等（stop 无槽可取、switch 收 invalid_request）。正常 stop 路径下 start 侧
  // 已先取走句柄，这里按 worker_id 核对后无操作。
  drop(dup_slot);
  drop(enc);
  clear_self(worker_id);
}

/// worker 退出时按身份释放自己的全局槽位（只删自己，不误删并发新起的 worker）。
fn clear_self(worker_id: u64) {
  let mut g = locked(&STARTED);
  if matches!(g.as_ref(), Some(h) if h.worker_id == worker_id) {
    *g = None; // drop 自身句柄（含 JoinHandle → detach；线程正在退出，无人 join）
  }
}

/// 换屏：先开新 duplication（失败则旧会话原样保留），输出尺寸变化才重建编码器
/// （take_state → import_state 保 seq 连续 + force_idr），最后原子换入。
#[allow(clippy::too_many_arguments)]
fn reopen_target(
  dup_slot: &mut Option<DuplicationOutput>,
  enc: &mut Encoder,
  rect: &mut (i32, i32, i32, i32),
  new_rect: (i32, i32, i32, i32),
  max_width: u32,
  fps: u32,
  bitrate: u32,
) -> AxResult<()> {
  let nd = DuplicationOutput::open(new_rect)?;
  let (ow, oh) = compute_out_size(new_rect.2 as u32, new_rect.3 as u32, max_width);
  if (ow, oh) != enc.out_size() {
    let state = enc.take_state();
    let codec = enc.codec(); // 重建沿用当前选型（HEVC 回退过就恒为实际生效的 H.264）
    // 先建新再换入：旧编码器存活期间 MF 计数不归零（不会触发中途 MFShutdown）。
    let mut fresh = Encoder::new(nd.device(), ow, oh, fps, bitrate, codec)?;
    fresh.import_state(state);
    fresh.force_idr();
    *enc = fresh;
  } else {
    enc.force_idr();
  }
  *dup_slot = Some(nd);
  *rect = new_rect;
  Ok(())
}

/// 采集链整体重建（capture-error 自愈）：重开 duplication（同矩形）并**无条件重建
/// 编码器**——新 duplication 携带新 D3D 设备，旧编码器绑死在旧设备上；状态机迁移
/// 保 seq 连续（IV = 盐∥seq 的唯一性契约），`force_idr` 保证重建点后首帧为关键帧。
/// 失败 = 调用方退出并上报 frozen（与旧行为同口径）。
fn rebuild_chain(
  dup_slot: &mut Option<DuplicationOutput>,
  enc: &mut Encoder,
  rect: (i32, i32, i32, i32),
  max_width: u32,
  fps: u32,
  bitrate: u32,
) -> AxResult<()> {
  let nd = DuplicationOutput::open(rect)?;
  let (ow, oh) = compute_out_size(rect.2 as u32, rect.3 as u32, max_width);
  let state = enc.take_state();
  let codec = enc.codec(); // 重建沿用当前选型
  // 先建新再换入：旧编码器存活期间 MF 计数不归零（不会触发中途 MFShutdown）。
  let mut fresh = Encoder::new(nd.device(), ow, oh, fps, bitrate, codec)?;
  fresh.import_state(state);
  fresh.force_idr();
  *enc = fresh;
  *dup_slot = Some(nd);
  Ok(())
}

/// 编码器单独重建（encode-error 自愈）：duplication/设备保持不变，仅换新编码器并
/// 迁移状态机（seq 连续 + force_idr → 重建点后首帧关键帧，对端可立即恢复）。
fn rebuild_encoder(
  dup: &DuplicationOutput,
  enc: &mut Encoder,
  rect: (i32, i32, i32, i32),
  max_width: u32,
  fps: u32,
  bitrate: u32,
) -> AxResult<()> {
  let (ow, oh) = compute_out_size(rect.2 as u32, rect.3 as u32, max_width);
  let state = enc.take_state();
  let codec = enc.codec(); // 重建沿用当前选型
  let mut fresh = Encoder::new(dup.device(), ow, oh, fps, bitrate, codec)?;
  fresh.import_state(state);
  fresh.force_idr();
  *enc = fresh;
  Ok(())
}

/// 取帧时刻（Unix epoch 毫秒，u32 截断）：帧龄只用百毫秒量级的差值，
/// 49.7 天回绕窗口内差值语义仍成立（回绕差 < 窗口一半）。时钟不可用回 0（= 未知）。
fn unix_ms_u32() -> u32 {
  std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .map(|d| d.as_millis() as u32)
    .unwrap_or(0)
}

fn send_status(
  tsfn: &ThreadsafeFunction<RdMediaStatusNapi, ErrorStrategy::Fatal>,
  frozen: bool,
  reason: Option<&str>,
) {
  let payload = RdMediaStatusNapi { frozen, reason: reason.map(str::to_string) };
  let _ = tsfn.call(payload, ThreadsafeFunctionCallMode::NonBlocking);
}
