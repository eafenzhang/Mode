//! 契约 A 的 napi 面（`rd_media_start / rd_media_switch / rd_media_stop`）与采集线程。
//!
//! 形状以计划「关键跨任务契约 A」为唯一权威，Task 7 按此写 JS：
//! - `onFrame/onStatus` 是**单参** JS 回调（`(frame) => void` / `(s) => void`）——
//!   故 TSFN 用 `ErrorStrategy::Fatal`（默认 CalleeHandled 会把 `(null, value)`
//!   塞给 JS，首参变 null，直接破坏 Task 7 的消费代码）；brief 代码块里
//!   `ThreadsafeFunction<T>` 未写第二泛型参数，默认策略与契约 A 的 TS 形状矛盾，
//!   此处以契约 A 的 TS 形状为准。
//! - 采集跑在专用 `std::thread`（简报 Step 4）；`start` 抢占式先 `stop`。
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use napi::bindgen_prelude::Buffer;
use napi::threadsafe_function::{ErrorStrategy, ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_derive::napi;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

use super::capture::{CaptureFrame, DuplicationOutput};
use super::encode::Encoder;
use crate::error::{AxError, AxResult};

/// 稀有异常路径的最小诊断面（本 crate 无日志设施）：只在 worker 出错退出、
/// stop 超时、switch 重开失败这类一次性事件打 stderr；不写帧内容/密钥/凭据。
fn warn(msg: &str) { eprintln!("[rd_media] {msg}"); }

#[napi(object)]
pub struct RdMediaRect {
  pub x: i32,
  pub y: i32,
  pub width: u32,
  pub height: u32,
}

/// 帧回调载荷：seq 单调（wire 的 IV = 会话盐∥seq，安全契约）、key = 码流含 IDR、
/// data = Annex-B ES（Task 7 起加密后进管道 5 字节头）。
#[napi(object)]
pub struct RdMediaFrameNapi {
  pub seq: u32,
  pub key: bool,
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

/// worker 句柄：stop 标志 + switch 待消费矩形 + join 句柄。
struct MediaWorkerHandle {
  stop: Arc<AtomicBool>,
  pending_rect: PendingRect,
  join: Option<JoinHandle<()>>,
}

/// 全局唯一会话槽（简报：`static STARTED: Mutex<Option<MediaWorkerHandle>>`）。
static STARTED: Mutex<Option<MediaWorkerHandle>> = Mutex::new(None);

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

/// 测试专用：无 start 时 stop 幂等（不触 DUPL/MF，无桌面会话也能跑）。
pub fn stop_for_tests() { stop(); }

/// 启动：抢占式先收旧会话 → 同步建 DUPL + 编码器（失败直接回错误给 JS，
/// 不把创建失败藏进线程）→ 起采集线程。
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
  stop(); // 已在跑先 stop 再起新线程（简报 Step 4）
  let dup = DuplicationOutput::open(rect)?;
  let (out_w, out_h) = compute_out_size(rect.2 as u32, rect.3 as u32, opts.max_width);
  let enc = Encoder::new(dup.device(), out_w, out_h, opts.fps, opts.bitrate_bps)?;
  let stop_flag = Arc::new(AtomicBool::new(false));
  let pending_rect = Arc::new(Mutex::new(None));
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
          rect, max_width, fps, bitrate, dup, enc, stop_f, pending, on_frame, on_status,
        )
      })
  };
  let join = spawn.map_err(|e| AxError::internal(format!("采集线程创建失败: {e}")))?;
  *locked(&STARTED) = Some(MediaWorkerHandle { stop: stop_flag, pending_rect, join: Some(join) });
  Ok(())
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
  dup: DuplicationOutput,
  mut enc: Encoder,
  stop: Arc<AtomicBool>,
  pending_rect: PendingRect,
  on_frame: ThreadsafeFunction<RdMediaFrameNapi, ErrorStrategy::Fatal>,
  on_status: ThreadsafeFunction<RdMediaStatusNapi, ErrorStrategy::Fatal>,
) {
  // MF/部分硬件 MFT 假定调用线程已初始化 COM——采集线程无 UI 需求，用 MTA。
  let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
  let com_inited = hr.0 == 0 || hr.0 == 1; // S_OK / S_FALSE；RPC_E_CHANGED_MODE 不置位
  let mut dup_slot: Option<DuplicationOutput> = Some(dup);
  let mut frozen = false;
  while !stop.load(Ordering::Relaxed) {
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
    let event = match frame {
      Ok(f) => f,
      Err(e) => {
        warn(&format!("采集循环错误，worker 退出: {e}"));
        break;
      }
    };
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
        match enc.push(&texture) {
          Ok(Some(chunk)) => {
            let payload = RdMediaFrameNapi {
              seq: chunk.seq,
              key: chunk.key,
              data: Buffer::from(chunk.data),
            };
            // NonBlocking（简报）：JS 消费慢只压 TSFN 队列，不反压采集线程。
            let _ = on_frame.call(payload, ThreadsafeFunctionCallMode::NonBlocking);
          }
          Ok(None) => {}
          Err(e) => {
            warn(&format!("编码循环错误，worker 退出: {e}"));
            break;
          }
        }
      }
    }
  }
  // 收口：dup/enc 在作用域尾随 drop（DUPL 释放 + MF drain/Shutdown 配对）。
  drop(dup_slot);
  drop(enc);
  if com_inited {
    unsafe { CoUninitialize() };
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
    // 先建新再换入：旧编码器存活期间 MF 计数不归零（不会触发中途 MFShutdown）。
    let mut fresh = Encoder::new(nd.device(), ow, oh, fps, bitrate)?;
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

fn send_status(
  tsfn: &ThreadsafeFunction<RdMediaStatusNapi, ErrorStrategy::Fatal>,
  frozen: bool,
  reason: Option<&str>,
) {
  let payload = RdMediaStatusNapi { frozen, reason: reason.map(str::to_string) };
  let _ = tsfn.call(payload, ThreadsafeFunctionCallMode::NonBlocking);
}
