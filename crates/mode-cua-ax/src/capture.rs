//! `capture` 原语：窗口 / 全屏截图（GDI → PNG），经 `uia_thread` 在 STA 线程串行执行。
//!
//! 语义（plan Task 4 Produces 节，spec「帧契约 / 风险表」）：
//! - 窗口：`PrintWindow(hwnd, PW_RENDERFULLCONTENT)` 渲染进 32bpp 顶向 DIB；失败回退从
//!   屏幕 DC 按窗口矩形 `BitBlt`（遮挡时抓到屏幕可见内容，兜底语义）。
//! - `fullScreen`：屏幕 DC `BitBlt` 虚拟屏（多显示器整屏）。
//! - `region`（相对窗口 / 屏幕左上角的 `[x,y,w,h]`）越界 clamp 到帧内并置 `clamped=true`；
//!   clamp 后无像素（全空）→ blank 错误。
//! - 空帧或抽样全黑 → `internal` "screen capture returned a blank frame"（抽样步长 = 宽/64
//!   逐行跳采；只看 BGR，忽略 DIB 中未定义的 alpha 位）。
//! - 错误映射（17 码）：窗口失效 → `element_unavailable`；缺 windowId / region 非四元组 →
//!   `invalid_request`；GDI / 编码失败 → `internal`；STA 往返超时 → `timeout`（submit 层）。
//!
//! GDI 调用全部经 `uia_thread` 在进程单例 STA 线程上执行（brief：与 observe 同线程串行，
//! 不另起线程）。
use crate::error::{AxError, AxResult};
use crate::uia_thread;
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder};
use napi::bindgen_prelude::Buffer;
use napi_derive::napi;
use std::ffi::c_void;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::UI::WindowsAndMessaging::{
  GetSystemMetrics, GetWindowRect, PW_RENDERFULLCONTENT, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
  SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

// 入参是请求面：Option 字段由 napi 呈现为可选（Task 2 口径，不加 use_nullable）。
#[napi(object)]
pub struct CaptureRequestNapi {
  pub window_id: Option<u32>,
  /// `[x, y, w, h]`，相对窗口（fullScreen 时相对屏幕左上角）。
  pub region: Option<Vec<i32>>,
  pub full_screen: bool,
}

// 出参全部必有字段；use_nullable 无 None 字段可落，按 Task 2 结果面口径保留标注。
#[napi(object, use_nullable = true)]
pub struct CaptureResultNapi {
  /// PNG 编码字节。
  pub data: Buffer,
  pub width: u32,
  pub height: u32,
  /// region 曾被 clamp 到帧内（任一边越界即为 true）。
  pub clamped: bool,
}

// napi 导出统一 js_name 钉 snake_case（Task 2 约定）；Rust 名与 capture() 内核区分。
#[napi(js_name = "capture")]
pub fn capture_napi(req: CaptureRequestNapi) -> napi::Result<CaptureResultNapi> {
  crate::to_napi(capture(req.window_id, req.region, req.full_screen))
}

/// 对外入口：校验在 STA 线程的 `run_capture` 内统一做，此处仅投递命令并同步等待
/// （30s 超时在 `uia_thread` 内映射为 `timeout`）。
pub fn capture(
  window_id: Option<u32>,
  region: Option<Vec<i32>>,
  full_screen: bool,
) -> AxResult<CaptureResultNapi> {
  // 入参先于 GDI 资源申请校验：region 形状非法直接拒绝，不做无谓截图。
  if let Some(r) = &region {
    if r.len() != 4 {
      return Err(AxError::new(
        "invalid_request",
        "region 必须为 [x, y, w, h] 四元组",
      ));
    }
  }
  uia_thread::submit_capture(window_id, region, full_screen)
}

/// STA 线程上执行的实际截图（由 `uia_thread` 调用）。
pub(crate) fn run_capture(
  window_id: Option<u32>,
  region: Option<Vec<i32>>,
  full_screen: bool,
) -> AxResult<CaptureResultNapi> {
  // 目标解析：fullScreen 优先（windowId 同时给出时忽略窗口，走屏幕 DC）。
  let (bgra, fw, fh) = if full_screen {
    grab_screen_frame()?
  } else if let Some(id) = window_id {
    grab_window_frame(id)?
  } else {
    return Err(AxError::new(
      "invalid_request",
      "fullScreen=false 时必须提供 windowId",
    ));
  };
  // region 相对帧左上角；缺省 = 整帧，不置 clamped。
  let (cx, cy, cw, ch, clamped) = match &region {
    None => (0, 0, fw, fh, false),
    Some(v) => clamp_region(v, fw, fh)?,
  };
  // 全空：clamp 后无像素可返回。
  if cw <= 0 || ch <= 0 {
    return Err(blank_err());
  }
  let cropped = crop_bgra(&bgra, fw, cx, cy, cw, ch);
  // 全黑抽样校验（对返回帧；防截屏 / 受保护内容黑帧）。
  ensure_non_blank(&cropped, cw, ch)?;
  let rgb = bgra_to_rgb(&cropped);
  let mut png = Vec::new();
  PngEncoder::new(&mut png)
    .write_image(&rgb, cw as u32, ch as u32, ExtendedColorType::Rgb8)
    .map_err(|e| AxError::internal(format!("PNG 编码失败: {e}")))?;
  Ok(CaptureResultNapi {
    data: Buffer::from(png),
    width: cw as u32,
    height: ch as u32,
    clamped,
  })
}

/// 全黑 / 全空统一错误（brief 指定字面量）。
fn blank_err() -> AxError {
  AxError::new("internal", "screen capture returned a blank frame")
}

/// region clamp：把 `[x,y,w,h]` 裁进帧 `[0,fw)×[0,fh)`，返回裁剪矩形与是否发生过调整。
/// 空交集不在这里报错（调用方按「全空」→ blank 处理）；四元组形状在入口校验。
fn clamp_region(region: &[i32], fw: i32, fh: i32) -> AxResult<(i32, i32, i32, i32, bool)> {
  if region.len() != 4 {
    return Err(AxError::new(
      "invalid_request",
      "region 必须为 [x, y, w, h] 四元组",
    ));
  }
  // i64 运算避免 i32 溢出（x+w 可达 2*MAX）。
  let (x, y, w, h) = (
    region[0] as i64,
    region[1] as i64,
    region[2] as i64,
    region[3] as i64,
  );
  let (fw64, fh64) = (fw as i64, fh as i64);
  let x0 = x.max(0);
  let y0 = y.max(0);
  let x1 = (x + w).min(fw64);
  let y1 = (y + h).min(fh64);
  let cw = x1 - x0;
  let ch = y1 - y0;
  // 任一边被改动即 clamped（负原点、越右/下界都算）。
  let clamped = x0 != x || y0 != y || cw != w || ch != h;
  Ok((x0 as i32, y0 as i32, cw as i32, ch as i32, clamped))
}

/// 从 BGRA 帧裁出子矩形（clamp 后的 x0+cw ≤ fw、y0+ch ≤ fh 由调用方保证）。
fn crop_bgra(frame: &[u8], fw: i32, x: i32, y: i32, w: i32, h: i32) -> Vec<u8> {
  let row_bytes = (w * 4) as usize;
  let mut out = Vec::with_capacity(row_bytes * h as usize);
  for row in y..y + h {
    let start = (row * fw + x) as usize * 4;
    out.extend_from_slice(&frame[start..start + row_bytes]);
  }
  out
}

/// 全黑抽样校验：步长 = 宽/64（至少 1 像素）逐行跳采，只看 BGR（DIB alpha 位未定义）。
/// 任一非黑样本即通过；抽完仍全 0（或帧为空）→ blank 错误。
fn ensure_non_blank(bgra: &[u8], w: i32, h: i32) -> AxResult<()> {
  if w <= 0 || h <= 0 || bgra.is_empty() {
    return Err(blank_err());
  }
  let step = ((w as usize) / 64).max(1);
  let width = w as usize;
  for y in 0..h as usize {
    let row = y * width * 4;
    let mut x = 0;
    while x < width {
      let i = row + x * 4;
      if (bgra[i] | bgra[i + 1] | bgra[i + 2]) != 0 {
        return Ok(());
      }
      x += step;
    }
  }
  Err(blank_err())
}

/// BGRA → RGB（丢 alpha、换 R/B）。
fn bgra_to_rgb(bgra: &[u8]) -> Vec<u8> {
  let mut rgb = Vec::with_capacity(bgra.len() / 4 * 3);
  for px in bgra.as_chunks::<4>().0 {
    rgb.extend_from_slice(&[px[2], px[1], px[0]]);
  }
  rgb
}

/// 截图窗口：`PrintWindow(PW_RENDERFULLCONTENT)` → 失败回退屏幕 DC 上的窗口矩形 BitBlt。
fn grab_window_frame(window_id: u32) -> AxResult<(Vec<u8>, i32, i32)> {
  // HWND 在 64 位进程里取低 32 位作窗口 id（与 apps::list_windows / observe 的截断互逆）。
  let hwnd = HWND(window_id as usize as *mut c_void);
  let mut rect = RECT::default();
  // unsafe 依据（块级）：windows 0.58 将 GetWindowRect 标为 unsafe（FFI 裸调用）；
  // 调用方 uia_thread 保证本函数只在进程单例 STA 线程执行，hwnd 只在本线程使用。
  unsafe {
    GetWindowRect(hwnd, &mut rect)
      .map_err(|e| AxError::new("element_unavailable", format!("GetWindowRect({window_id}) failed: {e}")))?;
  }
  let w = rect.right - rect.left;
  let h = rect.bottom - rect.top;
  if w <= 0 || h <= 0 {
    // 全空：退化窗口（如最小化到 0 尺寸）无像素可返回。
    return Err(blank_err());
  }
  let target = GdiTarget::new(w, h)?;
  // unsafe 依据（块级）：PrintWindow / BitBlt 均为 FFI；目标 DC 为本线程创建的内存 DC，
  // 位图节生命周期由 GdiTarget 守卫（Drop 先还原选入对象再释放）。
  unsafe {
    if !PrintWindow(hwnd, target.mem_dc, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)).as_bool() {
      // 回退：屏幕 DC 按窗口屏幕坐标 Blt（PrintWindow 对个别窗口失效时的兜底）。
      BitBlt(
        target.mem_dc,
        0,
        0,
        w,
        h,
        target.screen_dc,
        rect.left,
        rect.top,
        SRCCOPY,
      )
      .map_err(|e| {
        AxError::internal(format!("PrintWindow 失败且窗口矩形 BitBlt 回退失败: {e}"))
      })?;
    }
  }
  let frame = target.copy_pixels(w, h)?;
  Ok((frame, w, h))
}

/// 全屏截图：屏幕 DC BitBlt 虚拟屏（SM_*VIRTUALSCREEN 覆盖多显示器，原点可为负）。
fn grab_screen_frame() -> AxResult<(Vec<u8>, i32, i32)> {
  // unsafe 依据（块级）：GetSystemMetrics 为 FFI。
  let (x, y, w, h) = unsafe {
    (
      GetSystemMetrics(SM_XVIRTUALSCREEN),
      GetSystemMetrics(SM_YVIRTUALSCREEN),
      GetSystemMetrics(SM_CXVIRTUALSCREEN),
      GetSystemMetrics(SM_CYVIRTUALSCREEN),
    )
  };
  if w <= 0 || h <= 0 {
    return Err(blank_err());
  }
  let target = GdiTarget::new(w, h)?;
  unsafe {
    BitBlt(target.mem_dc, 0, 0, w, h, target.screen_dc, x, y, SRCCOPY)
      .map_err(|e| AxError::internal(format!("屏幕 BitBlt 失败: {e}")))?;
  }
  let frame = target.copy_pixels(w, h)?;
  Ok((frame, w, h))
}

/// 一块顶向 32bpp DIB 及其宿主 DC；Drop 按依赖顺序释放（先还原选入位图再删对象 / DC）。
struct GdiTarget {
  screen_dc: HDC,
  mem_dc: HDC,
  dib: HBITMAP,
  old: HGDIOBJ,
  bits: *mut c_void,
}

impl GdiTarget {
  fn new(w: i32, h: i32) -> AxResult<Self> {
    // unsafe 依据（块级）：GDI FFI；本函数仅在 STA 线程执行，句柄不跨线程。
    unsafe {
      let screen_dc = GetDC(None);
      if screen_dc.is_invalid() {
        return Err(AxError::internal("GetDC(NULL) 失败"));
      }
      let mem_dc = CreateCompatibleDC(screen_dc);
      if mem_dc.is_invalid() {
        let _ = ReleaseDC(None, screen_dc);
        return Err(AxError::internal("CreateCompatibleDC 失败"));
      }
      let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
          biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
          biWidth: w,
          // 负高 = 顶向 DIB：bits[0] 即顶行，免翻转。
          biHeight: -h,
          biPlanes: 1,
          biBitCount: 32,
          biCompression: BI_RGB.0,
          ..Default::default()
        },
        ..Default::default()
      };
      let mut bits: *mut c_void = std::ptr::null_mut();
      let dib = match CreateDIBSection(
        mem_dc,
        &bmi,
        DIB_RGB_COLORS,
        &mut bits,
        None,
        0,
      ) {
        Ok(d) => d,
        Err(e) => {
          let _ = DeleteDC(mem_dc);
          let _ = ReleaseDC(None, screen_dc);
          return Err(AxError::internal(format!("CreateDIBSection 失败: {e}")));
        }
      };
      let old = SelectObject(mem_dc, dib);
      if old.is_invalid() {
        let _ = DeleteObject(dib);
        let _ = DeleteDC(mem_dc);
        let _ = ReleaseDC(None, screen_dc);
        return Err(AxError::internal("SelectObject(DIB) 失败"));
      }
      Ok(Self {
        screen_dc,
        mem_dc,
        dib,
        old,
        bits,
      })
    }
  }

  /// 拷出整帧 BGRA（顶向，stride = w*4）；bits 异常时按「全空」报 blank 错误。
  fn copy_pixels(&self, w: i32, h: i32) -> AxResult<Vec<u8>> {
    if self.bits.is_null() || w <= 0 || h <= 0 {
      return Err(blank_err());
    }
    let len = (w as usize) * (h as usize) * 4;
    // unsafe 依据（块级）：bits 来自仍存活的 DIB 节（Drop 未执行），len 与 DIB 尺寸一致。
    Ok(unsafe { std::slice::from_raw_parts(self.bits as *const u8, len) }.to_vec())
  }
}

impl Drop for GdiTarget {
  fn drop(&mut self) {
    // unsafe 依据（块级）：GDI FFI 清理；还原选入位图后才允许 DeleteObject。
    unsafe {
      let _ = SelectObject(self.mem_dc, self.old);
      let _ = DeleteObject(self.dib);
      let _ = DeleteDC(self.mem_dc);
      let _ = ReleaseDC(None, self.screen_dc);
    }
  }
}
