//! DXGI Desktop Duplication 采集（spec「被控 Rust · 采集/光标」）。
//!
//! 所有 Win32/DXGI/D3D11 调用集中在本文件；seq/GOP/冻结状态机仍在 `state.rs`
//! （本文件不碰），保证纯逻辑可 `cargo test`。
//!
//! 光标合成路线（spec：光标烧进画面，`cursor-none` 语义）：DUPL 帧**不含**光标
//! （硬件光标由 DWM 叠加显示），必须自行合成。这里用「CPU 渲染形状（缓存）+
//! 小块 GPU 回读背景 + UpdateSubresource 回写」：
//! - 形状渲染是纯函数 `render_cursor_shape`（单色/颜色/掩码三种 `PointerShapeType`，
//!   无 Win32 依赖，单测覆盖），栅格在形状变更时渲染一次并缓存（原先每帧重算）；
//! - 半透明/掩码光标的透明像素必须保留桌面背景——直接整块 `UpdateSubresource`
//!   覆盖会在光标包围盒内留出色块（spec 验收项「对端光标在画面内随动」不能是色块），
//!   故先把补丁矩形从 **clean**（纯桌面内容）回读（`CopySubresourceRegion` 到 staging）
//!   再逐像素混合、写回 **proc**（合成输出）；
//! - 指针-only 更新（桌面无重绘，仅指针位置/形状/可见性变化）以 clean 重合成——
//!   静置画面上的光标移动由此可见（节流见 `POINTER_RECOMPOSE_MIN_INTERVAL`）；
//! - spec 的 GPU quad 路线是主案、本实现是等效的 CPU 混合变体；若未来要换 GPU quad，
//!   对外协议与 `CaptureFrame` 形状零改动（spec 降级预案同口径）。
use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{BOOL, FALSE, POINT, RECT, TRUE};
use windows::Win32::Graphics::Direct3D11::{
  ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_BIND_FLAG,
  D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX,
  D3D11_CPU_ACCESS_FLAG, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_FLAG, D3D11_MAP_READ,
  D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE,
  D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING, D3D11CreateDevice,
  ID3D11VideoContext, ID3D11VideoDevice, ID3D11VideoProcessor,
  ID3D11VideoProcessorEnumerator, ID3D11VideoProcessorInputView,
  ID3D11VideoProcessorOutputView, D3D11_TEX2D_VPIV, D3D11_TEX2D_VPOV,
  D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_CONTENT_DESC,
  D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0,
  D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0,
  D3D11_VIDEO_PROCESSOR_STREAM, D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
  D3D11_VPIV_DIMENSION_TEXTURE2D, D3D11_VPOV_DIMENSION_TEXTURE2D,
};
use windows::Win32::Graphics::Direct3D::{
  D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0,
};
use windows::Win32::Graphics::Gdi::{
  CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO,
  BITMAPINFOHEADER, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ,
};
use windows::Win32::Graphics::Dxgi::{
  CreateDXGIFactory1, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_WAIT_TIMEOUT,
  DXGI_OUTDUPL_FRAME_INFO, DXGI_OUTDUPL_POINTER_SHAPE_INFO,
  DXGI_OUTDUPL_POINTER_SHAPE_TYPE_COLOR, DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MASKED_COLOR,
  DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME, DXGI_OUTPUT_DESC, IDXGIOutput, IDXGIOutput1,
  IDXGIOutputDuplication, IDXGIFactory1, IDXGIResource,
};
use windows::Win32::Graphics::Dxgi::Common::{
  DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};
use windows::core::{Interface, Result as WResult};
use windows::Win32::UI::WindowsAndMessaging::{
  CURSORINFO, CURSOR_SHOWING, GetCursorInfo, GetIconInfo, HCURSOR, ICONINFO,
};

use crate::error::{AxError, AxResult};

use super::warn;

/// 光标形状缓存（`GetFramePointerShape` 原始缓冲 + 解析后的形状头 + 渲染栅格）。
struct ShapeCache {
  info: DXGI_OUTDUPL_POINTER_SHAPE_INFO,
  /// 原始形状缓冲（诊断保留；栅格渲染后不再参与热路径）。
  _data: Vec<u8>,
  /// 缓存命中的 `PointerShapeBufferSize`（形状变更检测键，简报要点 5）。
  buffer_size: u32,
  /// 渲染栅格（形状变更时计算一次；渲染失败为 None → 该形状周期内不打补丁）。
  raster: Option<CursorShapeRaster>,
}

/// user32 兜底的形状身份缓存（DUPL 零投递时启用）：按 HCURSOR 句柄命中则跳过
/// GetIconInfo/GetDIBits（那两个有 GDI 分配，形状低频才重建）——位置/可见性恒每帧采。
struct User32ShapeCache {
  handle: usize,
  /// 快照判重键（与 DUPL 的 `buffer_size` 同维度：形状身份变化 → 快照不等 → 重合成）。
  key: u32,
}

/// 指针状态快照（合成判重键）：位置 + 生效形状键 + 可见性。
/// 相等 = 自上次合成以来指针状态无新信息，不产帧。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PointerSnapshot {
  pub x: i32,
  pub y: i32,
  /// 生效形状键 = 缓存形状的 `buffer_size`（0 = 尚无形状）。
  pub shape: u32,
  pub visible: bool,
}

/// 指针-only 重合成的节流间隔（≈66fps 上限）：指针高频移动按事件率出帧会白耗
/// 编码/带宽；丢弃的中间更新不累积（指针状态是"最新值"语义，下一次 Acquire
/// 直接取到最新位置）。
const POINTER_RECOMPOSE_MIN_INTERVAL: Duration = Duration::from_millis(15);

/// 一帧采集结果（纹理是 GPU 句柄，seq 由外层 `state` 分配）。
pub enum CaptureFrame {
  /// 新桌面帧：已含光标补丁的 proc 纹理 + 光标补丁信息（供观测）。
  NewDesktop { texture: ID3D11Texture2D, cursor: Option<CursorPatch> },
  /// 无新帧（AcquireNextFrame 超时 / 指针无变化 / 指针节流窗口内）。
  Idle,
  /// 锁屏/安全桌面/会话切换：duplication 失效（触发 frozen；调用方须重开）。
  AccessLost,
}

/// 已合成好的光标补丁（`rgba` 为 **BGRA** 字节、与 proc 纹理格式一致，可直接
/// `UpdateSubresource`；字段名沿用简报，内容口径见 next_frame 文档）。
pub struct CursorPatch {
  pub x: i32,
  pub y: i32,
  pub w: u32,
  pub h: u32,
  pub rgba: Vec<u8>,
}

/// 形状渲染结果：BGRA 像素、每像素可见性掩码（1=光标实体，0=透明、露出背景）与
/// XOR 掩码（1 = MASKED_COLOR 的 0xFF 位：必须与**背景**异或后落笔。
/// GPU 双流覆盖层只表达 alpha 混合，表达不了 XOR——含 XOR 的形状由调用方
/// 整形状走 CPU 补丁路径，`xor` 只被 `blend_cursor_patch` 消费）。
pub struct CursorShapeRaster {
  pub w: u32,
  pub h: u32,
  pub bgra: Vec<u8>,
  pub visible: Vec<u8>,
  pub xor: Vec<u8>,
}

/// 单色光标（`PointerShapeType == 1`）：1bpp 位图、高度翻倍——前半 AND 掩码
/// （1=透明），后半颜色位（1=白/0=黑）。与 GDI 单色光标语义一致。
fn render_monochrome(w: u32, h: u32, pitch: u32, data: &[u8]) -> AxResult<CursorShapeRaster> {
  let need = (pitch as usize).checked_mul(h as usize * 2)
    .ok_or_else(|| AxError::internal("光标形状尺寸溢出"))?;
  if data.len() < need {
    return Err(AxError::internal("光标形状缓冲不足（单色）"));
  }
  let mask_stride = pitch as usize;
  let mut bgra = vec![0u8; (w * h * 4) as usize];
  let mut visible = vec![0u8; (w * h) as usize];
  for y in 0..h as usize {
    let and_row = &data[y * mask_stride..];
    let xor_row = &data[(h as usize + y) * mask_stride..];
    for x in 0..w as usize {
      let bit = 7 - (x % 8);
      let and_bit = (and_row[x / 8] >> bit) & 1;
      if and_bit == 1 {
        continue; // AND=1 → 透明（背景原样）
      }
      let i = (y * w as usize + x) * 4;
      let color = if (xor_row[x / 8] >> bit) & 1 == 1 { 255u8 } else { 0 };
      bgra[i] = color;
      bgra[i + 1] = color;
      bgra[i + 2] = color;
      bgra[i + 3] = 255;
      visible[y * w as usize + x] = 1;
    }
  }
  // 单色形状无 XOR 语义（XOR 半按普通黑白绘制——GDI 单色口径，既有行为保留）。
  Ok(CursorShapeRaster { w, h, bgra, visible, xor: vec![0u8; (w * h) as usize] })
}

/// 颜色光标（`PointerShapeType == 2`）：官方语义 = **32bpp ARGB，alpha 通道即形状**。
/// 2026-10-13 修正：此前强推 alpha=255 + 全可见，会把带透明通道的光标画成不透明色块
/// （实机光标位图 alpha 分布 880 透明/67 半透明/77 不透明——alpha 就是箭头形状）。
/// 无 alpha 信息的位图源不走此路径（user32 兜底对这类源用 AND 掩码求可见性）。
fn render_color(w: u32, h: u32, pitch: u32, data: &[u8]) -> AxResult<CursorShapeRaster> {
  let need = (pitch as usize).checked_mul(h as usize)
    .ok_or_else(|| AxError::internal("光标形状尺寸溢出"))?;
  if data.len() < need {
    return Err(AxError::internal("光标形状缓冲不足（颜色）"));
  }
  let mut bgra = vec![0u8; (w * h * 4) as usize];
  let mut visible = vec![0u8; (w * h) as usize];
  for y in 0..h as usize {
    let row = &data[y * pitch as usize..];
    for x in 0..w as usize {
      let src = &row[x * 4..x * 4 + 4];
      let i = (y * w as usize + x) * 4;
      bgra[i..i + 4].copy_from_slice(src); // 保留真实 alpha（overlay/补丁混合都消费它）
      visible[y * w as usize + x] = u8::from(src[3] > 0);
    }
  }
  Ok(CursorShapeRaster { w, h, bgra, visible, xor: vec![0u8; (w * h) as usize] })
}

/// 掩码颜色光标（`PointerShapeType == 4`）：官方语义 = **32bpp ARGB、alpha 位即掩码**
/// （仅允许 0/0xFF：0=RGB 替换屏幕像素、0xFF=RGB 与屏幕像素 XOR），**没有尾随 1bpp 掩码**。
/// 2026-10-13 修正：此前按「颜色位图 + 尾随 AND 掩码」解析，长度校验对真实缓冲必失败
/// → 该形状整体跳过 = 光标不可见（MSDN 原文「a 32 bpp ARGB format bitmap with the
/// mask value in the alpha bits」）。非 0/0xFF 的非法中间值宽容按替换处理。
fn render_masked_color(
  w: u32,
  h: u32,
  pitch: u32,
  data: &[u8],
) -> AxResult<CursorShapeRaster> {
  let need = (pitch as usize).checked_mul(h as usize)
    .ok_or_else(|| AxError::internal("光标形状尺寸溢出"))?;
  if data.len() < need {
    return Err(AxError::internal("光标形状缓冲不足（掩码颜色）"));
  }
  let pixels = (w * h) as usize;
  let mut bgra = vec![0u8; pixels * 4];
  // 掩码语义（0 替换 / 0xFF XOR）都要落笔：可见性恒 1，掩码位拆进 `xor`。
  let visible = vec![1u8; pixels];
  let mut xor = vec![0u8; pixels];
  for y in 0..h as usize {
    let row = &data[y * pitch as usize..];
    for x in 0..w as usize {
      let src = &row[x * 4..x * 4 + 4];
      let i = (y * w as usize + x) * 4;
      bgra[i..i + 4].copy_from_slice(&[src[0], src[1], src[2], 255]);
      if src[3] == 0xFF {
        xor[y * w as usize + x] = 1;
      }
    }
  }
  Ok(CursorShapeRaster { w, h, bgra, visible, xor })
}

/// 无 alpha 的颜色光标位图 + 1bpp AND 掩码（GetIconInfo 经典布局）→ 栅格。
/// AND=1 → 透明；其余按不透明替换（GDI 经典颜色光标无 XOR 语义）。
/// `mask_pitch` = 1bpp 行距（DIB 恒 4 字节对齐 = `div_ceil(32)*4`）。纯函数，单测覆盖。
fn render_color_with_mask(
  w: u32,
  h: u32,
  pitch: u32,
  bgra_data: &[u8],
  mask: &[u8],
  mask_pitch: u32,
) -> AxResult<CursorShapeRaster> {
  let need = (pitch as usize).checked_mul(h as usize);
  let mask_need = (mask_pitch as usize).checked_mul(h as usize);
  if need.is_none_or(|n| bgra_data.len() < n) || mask_need.is_none_or(|n| mask.len() < n) {
    return Err(AxError::internal("光标颜色位图/掩码缓冲不足"));
  }
  let mask_stride = mask_pitch as usize;
  let pixels = (w * h) as usize;
  let mut bgra = vec![0u8; pixels * 4];
  let mut visible = vec![0u8; pixels];
  for y in 0..h as usize {
    let row = &bgra_data[y * pitch as usize..];
    let and_row = &mask[y * mask_stride..];
    for x in 0..w as usize {
      let src = &row[x * 4..x * 4 + 4];
      let i = (y * w as usize + x) * 4;
      bgra[i..i + 4].copy_from_slice(&[src[0], src[1], src[2], 255]);
      let bit = 7 - (x % 8);
      visible[y * w as usize + x] = u8::from(((and_row[x / 8] >> bit) & 1) == 0);
    }
  }
  Ok(CursorShapeRaster { w, h, bgra, visible, xor: vec![0u8; pixels] })
}

/// 把 `GetFramePointerShape` 的原始形状渲染为 BGRA + 可见性掩码（**纯函数**，
/// 不碰 Win32——tests/rd_media.rs 覆盖三种形状类型与截断输入）。
///
/// 未知形状类型返回全不可见栅格（不打补丁、不杀采集流——光标形状是易变输入，
/// 不能让它决定管线生死）；缓冲不足返回 `internal` 错误。
pub fn render_cursor_shape(
  shape_type: u32,
  width: u32,
  height: u32,
  pitch: u32,
  data: &[u8],
) -> AxResult<CursorShapeRaster> {
  if width == 0 || height == 0 || pitch == 0 {
    return Ok(CursorShapeRaster {
      w: 0,
      h: 0,
      bgra: Vec::new(),
      visible: Vec::new(),
      xor: Vec::new(),
    });
  }
  if shape_type == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME.0 as u32 {
    // 行距下限防御：pitch 不足即数据布局非法，按截断处理（Err 而不是 OOB panic）。
    if pitch < width.div_ceil(8) {
      return Err(AxError::internal("光标形状行距不足（单色）"));
    }
    // 单色形状的 Height 是「AND+颜色」两倍，真实光标高度取半。
    render_monochrome(width, height / 2, pitch, data)
  } else if shape_type == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_COLOR.0 as u32
    || shape_type == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MASKED_COLOR.0 as u32
  {
    if pitch < width.saturating_mul(4) {
      return Err(AxError::internal("光标形状行距不足（颜色）"));
    }
    if shape_type == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_COLOR.0 as u32 {
      render_color(width, height, pitch, data)
    } else {
      render_masked_color(width, height, pitch, data)
    }
  } else {
    Ok(CursorShapeRaster { w: 0, h: 0, bgra: Vec::new(), visible: Vec::new(), xor: Vec::new() })
  }
}

/// user32 形状快照键（与 DUPL `buffer_size` 同维度的身份折叠）：HCURSOR 是系统光标
/// 身份，叠加栅格尺寸与长度防不同句柄折叠值撞车（撞车后果 = 快照误判"形状未变"，
/// 下一个不同形状仍会因句柄重建触发——最坏一次陈旧形状，不阻塞恢复）。
fn user32_shape_key(handle: usize, raster: &CursorShapeRaster) -> u32 {
  (handle as u32)
    ^ ((handle as u64 >> 32) as u32)
    ^ raster.w.rotate_left(8)
    ^ raster.h.rotate_left(16)
    ^ raster.bgra.len() as u32
}

/// 经 user32/GDI 读取光标形状（DUPL 零投递兜底的取字节层；仅 HCURSOR 变化时调用，
/// 形状低频）。返回（热区, 栅格）。三分支：
/// - 32bpp 颜色位图**有真 alpha** → COLOR 语义（alpha 即形状，见 `render_color`）；
/// - 32bpp 颜色位图无 alpha → 经典「颜色 + AND 掩码」（`render_color_with_mask`）；
/// - 无颜色位图（单色光标）→ 掩码位图 2 倍高 = AND+XOR 上下排，复用 MONOCHROME 布局。
///
/// 全部按 top-down DIB 取字节（`biHeight` 取负）；GDI 对象由守卫在退出时释放。
fn read_user32_shape(cursor: HCURSOR) -> AxResult<((i32, i32), CursorShapeRaster)> {
  let mut ii = ICONINFO::default();
  // unsafe 依据（块级）：GetIconInfo 为 user32 FFI；出参槽位由本栈帧持有，位图句柄
  // 所有权随 OwnedBitmap 守卫在本函数退出时 DeleteObject（含 Err 路径）。
  unsafe { GetIconInfo(cursor, &mut ii) }
    .map_err(|e| AxError::internal(format!("GetIconInfo 失败: {e}")))?;
  struct OwnedBitmap(HBITMAP);
  impl Drop for OwnedBitmap {
    fn drop(&mut self) {
      if self.0.is_invalid() {
        return;
      }
      // unsafe 依据（块级）：DeleteObject 为 GDI FFI；句柄来自 GetIconInfo（本函数持有）。
      let _ = unsafe { DeleteObject(HGDIOBJ(self.0.0)) };
    }
  }
  let color = OwnedBitmap(ii.hbmColor);
  let mask = OwnedBitmap(ii.hbmMask);
  let hot = (ii.xHotspot as i32, ii.yHotspot as i32);
  // unsafe 依据（块级）：CreateCompatibleDC 为 GDI FFI；DIB 读取只借 DC，读完即配对
  // DeleteDC（下方无条件执行，无提前返回路径）。
  let hdc = unsafe { CreateCompatibleDC(None) };
  if hdc.is_invalid() {
    return Err(AxError::internal("CreateCompatibleDC 失败"));
  }
  let raster = if !color.0.is_invalid() {
    read_color_cursor_dib(hdc, color.0, mask.0)
  } else {
    read_monochrome_cursor_dib(hdc, mask.0)
  };
  // unsafe 依据（块级）：DeleteDC 为 GDI FFI；与上方 CreateCompatibleDC 配对。
  let _ = unsafe { DeleteDC(hdc) };
  Ok((hot, raster?))
}

/// 32bpp 颜色光标位图读取（top-down）：有真 alpha → COLOR；无 alpha → 颜色+AND 掩码。
fn read_color_cursor_dib(hdc: HDC, color: HBITMAP, mask: HBITMAP) -> AxResult<CursorShapeRaster> {
  let mut bm = BITMAP::default();
  // unsafe 依据（块级）：GetObjectW 为 GDI FFI；出参槽位由本栈帧持有。
  let got = unsafe {
    GetObjectW(color, std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut c_void))
  };
  if got <= 0 {
    return Err(AxError::internal("光标颜色位图 GetObjectW 失败"));
  }
  if bm.bmBitsPixel != 32 || bm.bmWidth <= 0 || bm.bmHeight <= 0 {
    return Err(AxError::internal(format!(
      "不支持的光标颜色位图（{}x{} {}bpp）",
      bm.bmWidth, bm.bmHeight, bm.bmBitsPixel
    )));
  }
  let (w, h) = (bm.bmWidth as u32, bm.bmHeight as u32);
  let pitch = w * 4; // 32bpp 行距 DWORD 对齐 = w*4
  let mut pixels = vec![0u8; (pitch * h) as usize];
  let mut bi = BITMAPINFO {
    bmiHeader: BITMAPINFOHEADER {
      biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
      biWidth: bm.bmWidth,
      biHeight: -bm.bmHeight, // top-down：缓冲首行 = 顶行
      biPlanes: 1,
      biBitCount: 32,
      biCompression: 0,
      ..Default::default()
    },
    ..Default::default()
  };
  // unsafe 依据（块级）：GetDIBits 为 GDI FFI；缓冲长度按 w*h*4 分配、行数 = h。
  let lines = unsafe {
    GetDIBits(
      hdc,
      color,
      0,
      h,
      Some(pixels.as_mut_ptr() as *mut c_void),
      &mut bi,
      DIB_RGB_COLORS,
    )
  };
  if lines <= 0 {
    return Err(AxError::internal("光标颜色位图 GetDIBits 失败"));
  }
  if pixels.as_chunks::<4>().0.iter().any(|p| p[3] > 0) {
    return render_color(w, h, pitch, &pixels);
  }
  // 无 alpha 信息（经典 GDI 颜色位图）：透明性由 AND 掩码表达。
  if mask.is_invalid() {
    // 极端情形（无掩码）：按全可见不透明（宁可画出实体也不整形状消失）。
    let visible = vec![1u8; (w * h) as usize];
    return Ok(CursorShapeRaster {
      w,
      h,
      bgra: pixels,
      visible,
      xor: vec![0u8; (w * h) as usize],
    });
  }
  let (mask_bits, mask_pitch) = read_mask_dib_1bpp_rows(hdc, mask, h)?;
  render_color_with_mask(w, h, pitch, &pixels, &mask_bits, mask_pitch)
}

/// 单色光标：掩码位图 2 倍高（上半 AND、下半 XOR），复用 MONOCHROME 渲染（既有布局）。
fn read_monochrome_cursor_dib(hdc: HDC, mask: HBITMAP) -> AxResult<CursorShapeRaster> {
  if mask.is_invalid() {
    return Err(AxError::internal("单色光标无掩码位图"));
  }
  let mut bm = BITMAP::default();
  // unsafe 依据（块级）：GetObjectW 为 GDI FFI；出参槽位由本栈帧持有。
  let got = unsafe {
    GetObjectW(mask, std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut c_void))
  };
  if got <= 0 || bm.bmWidth <= 0 || bm.bmHeight <= 0 || bm.bmHeight % 2 != 0 {
    return Err(AxError::internal("单色光标掩码位图尺寸非法"));
  }
  let (w, total_h) = (bm.bmWidth as u32, bm.bmHeight as u32);
  let (bits, pitch) = read_mask_dib_1bpp_rows(hdc, mask, total_h)?;
  render_cursor_shape(DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME.0 as u32, w, total_h, pitch, &bits)
}

/// 1bpp 位图 top-down 读取（DIB BI_RGB 行距恒 4 字节对齐 = `div_ceil(32)*4`）。
fn read_mask_dib_1bpp_rows(hdc: HDC, hbm: HBITMAP, rows: u32) -> AxResult<(Vec<u8>, u32)> {
  let mut bm = BITMAP::default();
  // unsafe 依据（块级）：GetObjectW 为 GDI FFI；出参槽位由本栈帧持有。
  let got = unsafe {
    GetObjectW(hbm, std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut c_void))
  };
  if got <= 0 || bm.bmWidth <= 0 || bm.bmHeight <= 0 {
    return Err(AxError::internal("掩码位图 GetObjectW 失败"));
  }
  let pitch = (bm.bmWidth as u32).div_ceil(32) * 4;
  let mut bits = vec![0u8; (pitch * rows) as usize];
  let mut bi = BITMAPINFO {
    bmiHeader: BITMAPINFOHEADER {
      biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
      biWidth: bm.bmWidth,
      biHeight: -(rows as i32), // top-down
      biPlanes: 1,
      biBitCount: 1,
      biCompression: 0,
      ..Default::default()
    },
    ..Default::default()
  };
  // unsafe 依据（块级）：GetDIBits 为 GDI FFI；缓冲按 pitch*rows 分配、行数 = rows。
  let lines = unsafe {
    GetDIBits(
      hdc,
      hbm,
      0,
      rows,
      Some(bits.as_mut_ptr() as *mut c_void),
      &mut bi,
      DIB_RGB_COLORS,
    )
  };
  if lines <= 0 {
    return Err(AxError::internal("掩码位图 GetDIBits 失败"));
  }
  Ok((bits, pitch))
}

/// `AcquireNextFrame` 成功后的 Release 守卫：除 `DXGI_ERROR_ACCESS_LOST` 外，
/// 每个成功 acquire 分支都必须 Release（简报要点 6 纪律）。用 Drop 兜住所有
/// 提前返回（含 `?` 错误路径），失败只可能是设备已失效、由下一次 Acquire 暴露。
struct FrameRelease<'a>(&'a IDXGIOutputDuplication);

impl Drop for FrameRelease<'_> {
  fn drop(&mut self) {
    // unsafe 依据（块级）：ReleaseFrame 为 DXGI COM FFI；返回值忽略——释放失败
    // 只可能是设备已失效，由下一次 AcquireNextFrame 的 ACCESS_LOST 暴露。
    let _ = unsafe { self.0.ReleaseFrame() };
  }
}

/// COM 作用域（RAII）：进入时 `CoInitializeEx(COINIT_MULTITHREADED)`、离开配对
/// `CoUninitialize`。`CoInitializeEx`/`CoUninitialize` 必须同线程成对——用守卫
/// 保证提前返回（错误路径）也不漏配对。线程已初始化为别的模型（RPC_E_CHANGED_MODE）
/// 时不归本守卫管 → `enter()` 返回 None、不做 Uninitialize。
///
/// 为什么圈在 capture.rs：napi.rs 的 worker/start 只需要调这个**安全**入口，
/// Win32 unsafe 全部留在本文件（组织约束：Win32/DXGI/MF 调用圈在 capture/encode）。
pub(crate) struct ComScope(bool);

impl ComScope {
  pub(crate) fn enter() -> Option<Self> {
    // unsafe 依据（块级）：CoInitializeEx 为 COM FFI；S_OK(0)/S_FALSE(1) 表示本次
    // 调用成功（须配对 Uninitialize），其余（如 RPC_E_CHANGED_MODE）不配对。
    let hr = unsafe { windows::Win32::System::Com::CoInitializeEx(
      None,
      windows::Win32::System::Com::COINIT_MULTITHREADED,
    ) };
    if hr.0 == 0 || hr.0 == 1 { Some(Self(true)) } else { None }
  }
}

impl Drop for ComScope {
  fn drop(&mut self) {
    if self.0 {
      // unsafe 依据（块级）：与 enter() 同线程配对（RAII 保证）。
      unsafe { windows::Win32::System::Com::CoUninitialize() };
    }
  }
}

// ---- P0-2b：GPU 光标合成（双流 VideoProcessor，spec「光标」主案）----
//
// 目标：把「回读背景 + CPU 混合 + 回写」换成纯 GPU 双流合成——流 0 = 主画面
// （采集帧 / clean 底图，全幅），流 1 = 光标覆盖层（BGRA，透明像素 alpha=0），
// per-pixel alpha 由 VideoProcessorBlt 完成混合，目标 = proc。
// 初始化或运行中任一步在驱动上不被支持（VP 创建/输入视图/双流 Blt 失败）→ 整会话
// 回退 CPU 回读路径（build_cursor_patch），行为与 P0-2a 完全一致（spec 降级预案）。

/// 光标覆盖层的目标矩形 + 形状源偏移（CPU/GPU 两路共用的几何计算）。
struct CursorRect {
  x0: i32,
  y0: i32,
  w: u32,
  h: u32,
  sx: u32,
  sy: u32,
}

/// 光标覆盖层像素（纯函数）：可见 = 光标颜色（alpha 由渲染层保证 255），
/// 不可见 = 全透明（0,0,0,0）——VP 的 per-pixel alpha 混合据此保留桌面背景。
/// 注意：覆盖层**表达不了 XOR**（MASKED_COLOR 的 0xFF 掩码）——含 XOR 的形状在
/// `next_frame` 被整形状定向到 CPU 补丁路径，不会走到这里（形状层已分流）。
fn build_cursor_overlay(raster: &CursorShapeRaster) -> Vec<u8> {
  let pixels = (raster.w * raster.h) as usize;
  let mut out = Vec::with_capacity(pixels * 4);
  for i in 0..pixels {
    if raster.visible[i] == 1 {
      out.extend_from_slice(&raster.bgra[i * 4..i * 4 + 4]);
    } else {
      out.extend_from_slice(&[0, 0, 0, 0]);
    }
  }
  out
}

/// 光标覆盖层纹理（BGRA）与其输入视图；形状/尺寸变更时整体重建（低频）。
struct CursorOverlay {
  _texture: ID3D11Texture2D,
  input_view: ID3D11VideoProcessorInputView,
}

/// CPU 补丁混合（纯函数，`cursor_blend_tests` 锁四分支）：把形状按几何叠到
/// 补丁区域的背景上（`bg` = 背景矩形行优先 BGRA，长度 `rh*rw*4`，行距已是 `rw*4`——
/// 调用方从 staging 逐行压紧后传入）。分支优先级：
/// XOR（bg ^ color，MASKED_COLOR 0xFF 掩码）→ 真 alpha 混合（0<a<255，直通 alpha）
/// → 替换（a==255 或无 alpha 信息）→ 保留背景（不可见/全透明）。
/// 输出 alpha 通道不被下游消费（proc 随后转 NV12），恒置 255。
fn blend_cursor_patch(
  raster: &CursorShapeRaster,
  sx: usize,
  sy: usize,
  rw: usize,
  rh: usize,
  bg: &[u8],
) -> Vec<u8> {
  let mut out = Vec::with_capacity(rw * rh * 4);
  for r in 0..rh {
    let bg_row = &bg[r * rw * 4..(r + 1) * rw * 4];
    for c in 0..rw {
      let si = (sy + r) * raster.w as usize + (sx + c);
      let base = si * 4;
      let pixel = &raster.bgra[base..base + 4];
      let bg_px = &bg_row[c * 4..c * 4 + 4];
      if raster.xor.get(si).copied().unwrap_or(0) == 1 {
        // XOR 语义：RGB 与背景按位异或（α=0xFF 掩码的 0x00 位 → bg^0 = 保留背景）。
        out.extend_from_slice(&[
          bg_px[0] ^ pixel[0],
          bg_px[1] ^ pixel[1],
          bg_px[2] ^ pixel[2],
          255,
        ]);
      } else if raster.visible[si] == 1 {
        let a = pixel[3] as u32;
        if a == 255 {
          out.extend_from_slice(pixel);
        } else if a == 0 {
          out.extend_from_slice(bg_px);
        } else {
          // 直通 alpha 混合：src*a + bg*(255-a)（+127 四舍五入）。
          out.extend_from_slice(&[
            ((pixel[0] as u32 * a + bg_px[0] as u32 * (255 - a) + 127) / 255) as u8,
            ((pixel[1] as u32 * a + bg_px[1] as u32 * (255 - a) + 127) / 255) as u8,
            ((pixel[2] as u32 * a + bg_px[2] as u32 * (255 - a) + 127) / 255) as u8,
            255,
          ]);
        }
      } else {
        out.extend_from_slice(bg_px);
      }
    }
  }
  out
}

/// GPU 双流合成管线（每个 DuplicationOutput 一个；创建失败 → 会话走 CPU 回退）。
struct ComposePipeline {
  video_device: ID3D11VideoDevice,
  video_ctx: ID3D11VideoContext,
  enumerator: ID3D11VideoProcessorEnumerator,
  vp: ID3D11VideoProcessor,
  /// 合成目标（proc 纹理）的输出视图；proc 生命周期 = 会话，一次创建长期有效。
  output_view: ID3D11VideoProcessorOutputView,
  /// 主画面输入视图缓存（(纹理身份, 视图)）：采集帧纹理身份逐帧可能变化，
  /// clean 底图则恒定——按身份命中避免逐帧重建。
  main_input: Option<(usize, ID3D11VideoProcessorInputView)>,
  cursor: Option<CursorOverlay>,
  /// Blt 的帧序号（驱动要求单调）。
  frame_counter: u32,
}

impl ComposePipeline {
  fn create(
    device: &ID3D11Device,
    ctx: &ID3D11DeviceContext,
    target: &ID3D11Texture2D,
    out_w: u32,
    out_h: u32,
  ) -> AxResult<Self> {
    let video_device = device
      .cast::<ID3D11VideoDevice>()
      .map_err(|e| AxError::internal(format!("ID3D11VideoDevice QI 失败: {e}")))?;
    let video_ctx = ctx
      .cast::<ID3D11VideoContext>()
      .map_err(|e| AxError::internal(format!("ID3D11VideoContext QI 失败: {e}")))?;
    let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
      InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
      InputFrameRate: DXGI_RATIONAL { Numerator: 60, Denominator: 1 },
      InputWidth: out_w,
      InputHeight: out_h,
      OutputFrameRate: DXGI_RATIONAL { Numerator: 60, Denominator: 1 },
      OutputWidth: out_w,
      OutputHeight: out_h,
      Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
    };
    // unsafe 依据（块级）：D3D11 VideoDevice FFI；出参槽位由本栈帧持有，成功后引用归本结构。
    let enumerator = unsafe { video_device.CreateVideoProcessorEnumerator(&content) }
      .map_err(|e| AxError::internal(format!("合成 VP enumerator 创建失败: {e}")))?;
    let vp = unsafe { video_device.CreateVideoProcessor(&enumerator, 0) }
      .map_err(|e| AxError::internal(format!("合成 VP 创建失败: {e}")))?;
    let ov_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
      ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
      Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
        Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
      },
    };
    let mut output_view: Option<ID3D11VideoProcessorOutputView> = None;
    // unsafe 依据（块级）：同上 FFI；target（proc）生命周期 = 会话，视图长期有效。
    unsafe {
      video_device
        .CreateVideoProcessorOutputView(target, &enumerator, &ov_desc, Some(&mut output_view))
        .map_err(|e| AxError::internal(format!("合成输出视图创建失败: {e}")))?
    };
    // 流 1（光标覆盖层）的常量 alpha = 1.0：实际透明度完全取覆盖层的 per-pixel alpha。
    // unsafe 依据（块级）：ID3D11VideoContext FFI；vp 持有至结构释放。
    unsafe { video_ctx.VideoProcessorSetStreamAlpha(&vp, 1, TRUE, 1.0) };
    Ok(Self {
      video_device,
      video_ctx,
      enumerator,
      vp,
      output_view: output_view.ok_or_else(|| AxError::internal("合成输出视图为空"))?,
      main_input: None,
      cursor: None,
      frame_counter: 0,
    })
  }

  /// 形状变更时重建光标覆盖层（低频）：可见像素 = 光标颜色，其余全透明。
  fn update_cursor(
    &mut self,
    device: &ID3D11Device,
    ctx: &ID3D11DeviceContext,
    raster: &CursorShapeRaster,
  ) -> AxResult<()> {
    if raster.w == 0 || raster.h == 0 {
      self.cursor = None;
      return Ok(());
    }
    let bytes = build_cursor_overlay(raster);
    let texture = create_texture(
      device,
      raster.w,
      raster.h,
      DXGI_FORMAT_B8G8R8A8_UNORM,
      D3D11_USAGE_DEFAULT,
      D3D11_CPU_ACCESS_FLAG(0),
      D3D11_BIND_FLAG(0),
    )?;
    // unsafe 依据（块级）：UpdateSubresource 为 D3D11 FFI；bytes 逐行 w*4 与纹理等宽。
    unsafe {
      ctx.UpdateSubresource(
        &texture,
        0,
        None,
        bytes.as_ptr() as *const c_void,
        raster.w * 4,
        0,
      )
    };
    let iv_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
      FourCC: 0,
      ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
      Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
        Texture2D: D3D11_TEX2D_VPIV { MipSlice: 0, ArraySlice: 0 },
      },
    };
    let mut view: Option<ID3D11VideoProcessorInputView> = None;
    // unsafe 依据（块级）：同上 FFI；texture 由本结构持有，视图引用其生命周期。
    unsafe {
      self
        .video_device
        .CreateVideoProcessorInputView(&texture, &self.enumerator, &iv_desc, Some(&mut view))
        .map_err(|e| AxError::internal(format!("光标覆盖层输入视图失败: {e}")))?
    };
    self.cursor = Some(CursorOverlay {
      _texture: texture,
      input_view: view.ok_or_else(|| AxError::internal("光标输入视图为空"))?,
    });
    Ok(())
  }

  /// 形状渲染失败：本形状周期内不叠覆盖层（与 CPU 路径的「跳过补丁」同语义）。
  fn clear_cursor(&mut self) {
    self.cursor = None;
  }

  /// 主画面输入视图（按纹理身份缓存）。
  fn main_input_view(
    &mut self,
    device: &ID3D11Device,
    texture: &ID3D11Texture2D,
  ) -> AxResult<ID3D11VideoProcessorInputView> {
    let identity = texture.as_raw() as usize;
    if let Some((cached_identity, view)) = &self.main_input {
      if *cached_identity == identity {
        return Ok(view.clone());
      }
    }
    let iv_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
      FourCC: 0,
      ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
      Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
        Texture2D: D3D11_TEX2D_VPIV { MipSlice: 0, ArraySlice: 0 },
      },
    };
    let mut view: Option<ID3D11VideoProcessorInputView> = None;
    // unsafe 依据（块级）：D3D11 VideoDevice FFI；纹理由调用方（采集帧/clean）持有至 Blt 提交。
    unsafe {
      device
        .cast::<ID3D11VideoDevice>()
        .map_err(|e| AxError::internal(format!("ID3D11VideoDevice QI 失败: {e}")))?
        .CreateVideoProcessorInputView(texture, &self.enumerator, &iv_desc, Some(&mut view))
        .map_err(|e| AxError::internal(format!("主画面输入视图失败: {e}")))?
    };
    let view = view.ok_or_else(|| AxError::internal("主画面输入视图为空"))?;
    self.main_input = Some((identity, view.clone()));
    Ok(view)
  }

  /// 双流合成：Blt(主画面 + 光标覆盖层) → proc。光标不可见/无覆盖层 → 单流（纯拷贝）。
  fn compose(
    &mut self,
    device: &ID3D11Device,
    source: &ID3D11Texture2D,
    cursor: Option<&CursorRect>,
  ) -> AxResult<()> {
    let main_view = self.main_input_view(device, source)?;
    let overlay = match (self.cursor.as_ref(), cursor) {
      (Some(overlay), Some(rect)) => {
        // 光标流的目标矩形 = 覆盖矩形；源矩形 = 形状内偏移（越界钳制后的子矩形）。
        let dest = RECT {
          left: rect.x0,
          top: rect.y0,
          right: rect.x0 + rect.w as i32,
          bottom: rect.y0 + rect.h as i32,
        };
        let src = RECT {
          left: rect.sx as i32,
          top: rect.sy as i32,
          right: (rect.sx + rect.w) as i32,
          bottom: (rect.sy + rect.h) as i32,
        };
        // unsafe 依据（块级）：ID3D11VideoContext FFI；RECT 为本调用临时值。
        unsafe {
          self
            .video_ctx
            .VideoProcessorSetStreamDestRect(&self.vp, 1, TRUE, Some(&dest));
          self
            .video_ctx
            .VideoProcessorSetStreamSourceRect(&self.vp, 1, TRUE, Some(&src));
        }
        Some(overlay.input_view.clone())
      }
      _ => {
        // 无覆盖层：禁用流 1（保持单流纯拷贝；dest 由上次残值改为失效）。
        // unsafe 依据（块级）：同上 FFI。
        unsafe { self.video_ctx.VideoProcessorSetStreamDestRect(&self.vp, 1, FALSE, None) };
        None
      }
    };
    let mut streams = vec![D3D11_VIDEO_PROCESSOR_STREAM {
      Enable: TRUE,
      pInputSurface: ManuallyDrop::new(Some(main_view)),
      ..Default::default()
    }];
    if let Some(view) = overlay {
      streams.push(D3D11_VIDEO_PROCESSOR_STREAM {
        Enable: TRUE,
        pInputSurface: ManuallyDrop::new(Some(view)),
        ..Default::default()
      });
    }
    // unsafe 依据（块级）：VideoProcessorBlt 为 COM FFI；流数组在调用期间存活，
    // 输入面引用由视图持有（主画面源纹理由调用方在 Blt 提交前保持存活）。
    unsafe {
      self
        .video_ctx
        .VideoProcessorBlt(&self.vp, &self.output_view, self.frame_counter, &streams)
        .map_err(|e| AxError::internal(format!("合成 Blt 失败: {e}")))?
    };
    self.frame_counter = self.frame_counter.wrapping_add(1);
    Ok(())
  }
}

/// 单个输出的 duplication 会话：D3D11 device + clean/proc 纹理对 + DXGI 复制。
pub struct DuplicationOutput {
  device: ID3D11Device,
  ctx: ID3D11DeviceContext,
  /// `None` = 已 close（释放 duplication，简报要点 6）。
  duplication: Option<IDXGIOutputDuplication>,
  /// 目标输出的物理矩形 (x, y, w, h)：光标屏幕坐标 → 纹理坐标的换算基准。
  desktop_rect: (i32, i32, i32, i32),
  /// 纯桌面内容（不含光标）：补丁背景回读与指针-only 重合成的源。
  clean: ID3D11Texture2D,
  /// 合成输出（clean + 光标补丁）：喂编码器的纹理（CopyResource/UpdateSubresource 目标）。
  proc: ID3D11Texture2D,
  /// 光标背景回读用 staging（STAGING/CPU_READ，同 desc）。
  bg_staging: ID3D11Texture2D,
  shape: Option<ShapeCache>,
  /// `clean` 是否已装载过真实桌面帧（首个桌面帧之前的指针-only 帧不可重合成）。
  clean_valid: bool,
  /// 上次合成（桌面帧或指针-only）时的指针快照（相同 = 无新信息，不产帧）。
  last_composited: Option<PointerSnapshot>,
  /// 上次合成时间：指针-only 重合成按 `POINTER_RECOMPOSE_MIN_INTERVAL` 节流
  /// （桌面帧不受限——它由内容驱动，光标随帧携带）。
  last_compose_at: Option<Instant>,
  /// GPU 光标合成管线（P0-2b）；`None` = 驱动不支持/运行中失败 → CPU 回读路径。
  compose: Option<ComposePipeline>,
  /// DUPL 是否投递过光标形状：false（会话内从未投递）= 软件光标/虚拟显示环境
  /// （IDD 虚拟屏、远控软件接管显示）——实机探针 200+ 次指针移动
  /// `LastMouseUpdateTime`/`PointerShapeBufferSize` 恒 0，此时指针状态切 user32 兜底。
  dupl_shape_seen: bool,
  /// user32 兜底的形状句柄缓存（`None` = 尚未取到形状或该句柄取形失败）。
  user32_shape: Option<User32ShapeCache>,
}

impl DuplicationOutput {
  /// 枚举首个 `DesktopCoordinates` 命中 rect 的输出并建 D3D11 device + duplication。
  ///
  /// 失败语义（spec 失败表）：`DuplicateOutput` 失败 = 能力面缺失 → `unimplemented`
  /// （宿主映射 `unsupported`）；device/纹理等其它失败 → `internal`。
  pub fn open(rect: (i32, i32, i32, i32)) -> AxResult<Self> {
    if rect.2 <= 0 || rect.3 <= 0 {
      return Err(AxError::new("invalid_request", "采集矩形宽高必须为正"));
    }
    // 要点 1：硬件 D3D11（feature level ≥ 11_0），失败回退 WARP（软光栅仍满足
    // 纹理拷贝/回读需求，避免旧 GPU 直接判能力缺失）。
    let (device, ctx) = create_device()?;
    let duplication = unsafe { open_duplication(&device, rect)? };
    // clean/proc/staging 尺寸取真实输出矩形（与 acquire 到的帧纹理同尺寸、同格式——
    // DUPL 帧恒为 B8G8R8A8_UNORM）。
    let (w, h) = (rect.2 as u32, rect.3 as u32);
    // bind 同时带 RENDER_TARGET（语义上可作拷贝目标）与 SHADER_RESOURCE（编码链会为
    // proc 建 VP 输入视图，视图类型按实现可能要求 SRV 位）——多带无害、少带会在部分
    // 驱动上直接 CreateVideoProcessorInputView 失败。clean 同 desc（拷贝源/回读源）。
    let clean = create_texture(
      &device,
      w,
      h,
      DXGI_FORMAT_B8G8R8A8_UNORM,
      D3D11_USAGE_DEFAULT,
      D3D11_CPU_ACCESS_FLAG(0),
      D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE,
    )?;
    let proc = create_texture(
      &device,
      w,
      h,
      DXGI_FORMAT_B8G8R8A8_UNORM,
      D3D11_USAGE_DEFAULT,
      D3D11_CPU_ACCESS_FLAG(0),
      D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE,
    )?;
    let bg_staging = create_texture(
      &device,
      w,
      h,
      DXGI_FORMAT_B8G8R8A8_UNORM,
      D3D11_USAGE_STAGING,
      D3D11_CPU_ACCESS_READ,
      D3D11_BIND_FLAG(0),
    )?;
    // GPU 光标合成（P0-2b）：初始化失败（驱动不支持 VP 双流/输入视图）即整会话回退
    // CPU 回读路径——P0-2a 行为原样保留，不影响采集可用性（spec 降级预案）。
    let compose = match ComposePipeline::create(&device, &ctx, &proc, w, h) {
      Ok(pipeline) => Some(pipeline),
      Err(e) => {
        warn(&format!("GPU 光标合成不可用，回退 CPU 回读路径: {e}"));
        None
      }
    };
    Ok(Self {
      device,
      ctx,
      duplication: Some(duplication),
      desktop_rect: rect,
      clean,
      proc,
      bg_staging,
      shape: None,
      clean_valid: false,
      last_composited: None,
      last_compose_at: None,
      compose,
      dupl_shape_seen: false,
      user32_shape: None,
    })
  }

  /// 采集端持有的 D3D11 device（编码链 `Encoder::new` 复用同一 device，
  /// 保证 VP 输入视图与 proc 纹理同设备）。
  pub fn device(&self) -> &ID3D11Device { &self.device }

  /// 关闭：释放 duplication（COM 引用随 drop 归零）；幂等。
  pub fn close(&mut self) { self.duplication = None; }

  /// 主动重发最近一帧（键请求/开流补帧，2026-10-14）：
  /// 桌面与光标都没有变化时，`next_frame` 只会返回 Idle（正常环境 `dupl_shape_seen` 下
  /// 连 user32 补帧都不走），调用方 `enc.force_idr()` 便永远没有帧可编 → 对端拿到零帧、
  /// 表现为「画面已连接 / 0 fps / 0.0 Mbps」的纯黑（实机实证）。
  /// 这里把已合成的 `proc` 原样再交出去一次：内容就是当前屏幕，编码器照常走一次 IDR。
  /// 尚未装载过底图（首个桌面帧未到）时返回 Idle——不凭空造帧，交给后续真实变化兑现。
  pub fn reemit(&mut self) -> AxResult<CaptureFrame> {
    if !self.clean_valid {
      return Ok(CaptureFrame::Idle);
    }
    Ok(CaptureFrame::NewDesktop { texture: self.proc.clone(), cursor: None })
  }

  /// 阻塞至下一帧（`timeout_ms` 典型 16ms）。返回前已完成合成：
  /// `clean` ← 桌面帧（纯内容），`proc` = clean + 光标补丁（`cursor.rgba` 为
  /// **BGRA** 的已混合字节）。三类事件：
  /// - 桌面更新（present/累积帧）：拷贝新内容 + 合成光标；
  /// - 指针-only 更新（无桌面重绘，仅指针位置/形状/可见性变化）：以最近桌面内容
  ///   重合成并产帧——**静置画面上的光标移动由此可见**；
  /// - 其余（指针节流窗口内 / 无任何变化 / 尚无底图）→ Idle（指针"最新值"语义，
  ///   丢弃的中间位置不累积，下一次 Acquire 直接取到最新）。
  pub fn next_frame(&mut self, timeout_ms: u32) -> AxResult<CaptureFrame> {
    let Some(dup) = self.duplication.clone() else {
      // close 之后再来调用是编程错误（worker 循环用 Option<DuplicationOutput>
      // 表达冻结，不会走到这里）。
      return Err(AxError::internal("duplication 已关闭"));
    };
    let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
    let mut resource: Option<IDXGIResource> = None;
    // unsafe 依据（块级）：AcquireNextFrame 为 DXGI COM FFI；info/resource 为
    // 本栈帧槽位，resource 的所有权在成功后由本函数接管（cast 后引用）。
    match unsafe { dup.AcquireNextFrame(timeout_ms, &mut info, &mut resource) } {
      Ok(()) => {}
      // AccessLost：不 Release（帧已不持有；简报要点 3——设备侧已丢弃）。
      Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST => return Ok(CaptureFrame::AccessLost),
      // 超时 = 该 tick 无新帧。
      Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => {
        // DUPL 零投递环境（软件光标/虚拟显示）**没有指针事件**唤醒指针-only 分支：
        // 超时改由 user32 快照轮询补帧——光标移动必须自己产帧（判重 + 节流与 DUPL
        // pointer-only 同语义），否则远控侧动鼠标画面纹丝不动（2026-10-13 实测根因：
        // 只有桌面像素恰好变化才出帧，会话看起来「卡死」）。正常环境不走此路。
        return self.poll_user32_pointer_frame();
      }
      Err(e) => {
        return Err(AxError::internal(format!("AcquireNextFrame 失败: {e}")));
      }
    }
    // 从这里开始所有分支都由守卫保证 Release（含 Idle 提前返回与 `?` 错误路径）。
    let _rel = FrameRelease(&dup);
    // 形状缓存：buffer_size=0 = 自上次以来形状未变（MS 样例口径），保留缓存不刷新。
    self.refresh_shape(&dup, info.PointerShapeBufferSize)?;
    // 指针状态解析（每帧一次，合成与判重统一用它）：DUPL 权威；DUPL 零投递 → user32 兜底。
    let snap = self.resolve_pointer(&info);
    let has_desktop_update = info.LastPresentTime != 0 || info.AccumulatedFrames != 0;
    if has_desktop_update {
      let res = resource.ok_or_else(|| AxError::internal("AcquireNextFrame 未返回桌面纹理"))?;
      let frame = res.cast::<ID3D11Texture2D>().map_err(|e| {
        AxError::internal(format!("桌面资源转 ID3D11Texture2D 失败: {e}"))
      })?;
      // clean ← frame：CPU 路径（GPU 初始回退 / 运行时回退 / XOR 形状定向）的底图与
      // 背景都从它来；GPU 成功路径同样保留（指针-only 重合成的源）。一次拷贝两路共用。
      // （P0-3 曾把它收进 GPU 分支内——compose 初始即 None 的 CPU 回退会拿旧底图出帧，
      // 桌面内容冻结；此处恢复为无条件拷贝。）
      // unsafe 依据（块级）：CopyResource 为 D3D11 FFI；clean/frame 同设备同尺寸同格式。
      unsafe { self.ctx.CopyResource(&self.clean, &frame) };
      self.clean_valid = true;
      // GPU 路径（P0-2b/P0-3）：主画面直接以采集帧纹理进 Blt，省 frame→clean→proc 第二拷。
      // 含 XOR 的形状（MASKED_COLOR 0xFF 掩码）双流覆盖层表达不了 → 定向走下方 CPU 补丁
      //（按形状分流，非失败回退，不告警）。
      if self.compose.is_some() && !self.shape_needs_cpu() {
        match self.compose_frame_gpu(&frame, &snap) {
          Ok(()) => {
            self.last_composited = Some(snap);
            self.last_compose_at = Some(Instant::now());
            return Ok(CaptureFrame::NewDesktop { texture: self.proc.clone(), cursor: None });
          }
          Err(e) => {
            warn(&format!("GPU 光标合成失败，本会话回退 CPU 回读路径: {e}"));
            self.compose = None; // 落到下方 CPU 路径（clean 已就绪）
          }
        }
      }
      // unsafe 依据（块级）：CopyResource 为 D3D11 FFI；clean/proc 同设备同尺寸同格式。
      unsafe { self.ctx.CopyResource(&self.proc, &self.clean) };
      // 光标补丁（从 clean 回读背景 + 混合 + 写回 proc；无可见光标 → None）。
      let cursor = self.build_cursor_patch(&snap)?;
      self.last_composited = Some(snap);
      self.last_compose_at = Some(Instant::now());
      return Ok(CaptureFrame::NewDesktop { texture: self.proc.clone(), cursor });
    }
    // 指针-only：无桌面更新，仅指针状态可能变化。没有底图 / 状态与上次合成
    // 完全相同 → 无事可做。
    if !self.clean_valid {
      return Ok(CaptureFrame::Idle);
    }
    if self.last_composited == Some(snap) {
      return Ok(CaptureFrame::Idle);
    }
    // 节流：睡到窗口结束再重取（不睡会按指针事件率忙旋；期间桌面 present 会
    // 累积在下一次 Acquire，指针位置取最新值）。
    if let Some(last) = self.last_compose_at {
      let elapsed = last.elapsed();
      if elapsed < POINTER_RECOMPOSE_MIN_INTERVAL {
        std::thread::sleep(POINTER_RECOMPOSE_MIN_INTERVAL - elapsed);
      }
    }
    // 以最近桌面内容重合成：擦掉上一帧烧入的旧光标，再按新指针状态合成。
    // GPU 路径：Blt(clean + 光标覆盖层) 全幅写出 proc（无需预拷贝）；失败回退 CPU；
    // 含 XOR 的形状同样按下方 CPU 路径分流。
    if self.compose.is_some() && !self.shape_needs_cpu() {
      let base = self.clean.clone();
      let gpu = self.compose_frame_gpu(&base, &snap);
      match gpu {
        Ok(()) => {
          self.last_composited = Some(snap);
          self.last_compose_at = Some(Instant::now());
          return Ok(CaptureFrame::NewDesktop { texture: self.proc.clone(), cursor: None });
        }
        Err(e) => {
          warn(&format!("GPU 光标合成失败，本会话回退 CPU 回读路径: {e}"));
          self.compose = None;
        }
      }
    }
    unsafe { self.ctx.CopyResource(&self.proc, &self.clean) };
    let cursor = self.build_cursor_patch(&snap)?;
    self.last_composited = Some(snap);
    self.last_compose_at = Some(Instant::now());
    Ok(CaptureFrame::NewDesktop { texture: self.proc.clone(), cursor })
  }

  /// 当前形状含 XOR 像素（MASKED_COLOR 的 0xFF 掩码）→ 定向走 CPU 补丁路径：
  /// 双流覆盖层只表达 alpha 混合，表达不了「与背景异或」（非失败，不告警）。
  fn shape_needs_cpu(&self) -> bool {
    self
      .shape
      .as_ref()
      .and_then(|s| s.raster.as_ref())
      .is_some_and(|r| r.xor.contains(&1))
  }

  /// 每帧解析指针状态（合成与判重的唯一输入）：DUPL 已投递过形状 → 权威取帧内字段；
  /// 否则（软件光标/虚拟显示环境：IDD 虚拟屏、远控接管显示——DUPL 零指针投递）→
  /// user32 兜底采样。快照的形状键：DUPL 用 `buffer_size`，user32 用句柄折叠键。
  fn resolve_pointer(&mut self, info: &DXGI_OUTDUPL_FRAME_INFO) -> PointerSnapshot {
    if self.dupl_shape_seen {
      return PointerSnapshot {
        x: info.PointerPosition.Position.x,
        y: info.PointerPosition.Position.y,
        shape: self.shape.as_ref().map_or(0, |s| s.buffer_size),
        visible: info.PointerPosition.Visible.as_bool(),
      };
    }
    self.resolve_user32_pointer()
  }

  /// user32 兜底采样（DUPL 零投递时的唯一指针源）：位置/可见性恒取 `GetCursorInfo`
  /// （无分配）；形状按 HCURSOR 句柄缓存，句柄变化才 `GetIconInfo`/`GetDIBits` 重建
  /// （GDI 有分配，形状低频），落位与 GPU 覆盖层同步复用 `install_shape`（与 DUPL
  /// 同一出口）。取形失败缓存为「无形状」（按句柄率限告警，不逐帧刷屏）；
  /// 无形状时返回**稳定隐藏快照**——没有光标可画，避免空指针移动产出无变化帧。
  fn resolve_user32_pointer(&mut self) -> PointerSnapshot {
    const NO_SHAPE: PointerSnapshot = PointerSnapshot { x: 0, y: 0, shape: 0, visible: false };
    let mut ci = CURSORINFO {
      cbSize: std::mem::size_of::<CURSORINFO>() as u32,
      ..Default::default()
    };
    // unsafe 依据（块级）：GetCursorInfo 为 user32 FFI；ci 为本栈槽位。
    if unsafe { GetCursorInfo(&mut ci) }.is_err() || (ci.flags.0 & CURSOR_SHOWING.0) == 0 {
      return NO_SHAPE;
    }
    let handle = ci.hCursor.0 as usize;
    if self.user32_shape.as_ref().map(|c| c.handle) != Some(handle) {
      // 形状缓存未命中（首次 / 光标类型切换）→ 取形重建。
      match read_user32_shape(ci.hCursor) {
        Ok((hot, raster)) => {
          let key = user32_shape_key(handle, &raster);
          self.install_shape(
            DXGI_OUTDUPL_POINTER_SHAPE_INFO {
              Type: DXGI_OUTDUPL_POINTER_SHAPE_TYPE_COLOR.0 as u32,
              Width: raster.w,
              Height: raster.h,
              Pitch: 0,
              HotSpot: POINT { x: hot.0, y: hot.1 },
            },
            Vec::new(),
            key,
            Some(raster),
          );
          self.user32_shape = Some(User32ShapeCache { handle, key });
        }
        Err(e) => {
          // 按句柄率限：缓存为无形状后，同句柄不再重试/告警，句柄变化才再试。
          warn(&format!("user32 光标取形失败，本形状周期内不画光标: {e}"));
          self.install_shape(DXGI_OUTDUPL_POINTER_SHAPE_INFO::default(), Vec::new(), 0, None);
          self.user32_shape = Some(User32ShapeCache { handle, key: 0 });
          return NO_SHAPE;
        }
      }
    }
    let Some(cache) = self.user32_shape.as_ref() else {
      return NO_SHAPE;
    };
    if cache.key == 0 {
      // 取形失败缓存：无形状可画，稳定隐藏快照（避免逐帧空合成）。
      return NO_SHAPE;
    }
    PointerSnapshot {
      x: ci.ptScreenPos.x,
      y: ci.ptScreenPos.y,
      shape: cache.key,
      visible: true,
    }
  }

  /// 零投递环境的指针运动补帧（Acquire 超时时调用）：DUPL 不投递指针事件，
  /// 「动鼠标出帧」只能靠 user32 快照轮询触发——与 DUPL 指针-only 分支完全同语义：
  /// 快照判重（无变化 → Idle）→ `POINTER_RECOMPOSE_MIN_INTERVAL` 节流 → 以 clean
  /// 重合成（GPU 覆盖层 / CPU 补丁，含 XOR 分流）→ 更新判重键。
  /// 正常环境（`dupl_shape_seen`）不走此路：指针事件仍由 DUPL 唤醒，行为不变。
  fn poll_user32_pointer_frame(&mut self) -> AxResult<CaptureFrame> {
    if self.dupl_shape_seen || !self.clean_valid {
      return Ok(CaptureFrame::Idle);
    }
    let snap = self.resolve_user32_pointer();
    if self.last_composited == Some(snap) {
      return Ok(CaptureFrame::Idle);
    }
    // 节流与 DUPL 指针-only 同源（超时本身已等 ≥16ms，命中窗口的概率低）。
    if let Some(last) = self.last_compose_at {
      let elapsed = last.elapsed();
      if elapsed < POINTER_RECOMPOSE_MIN_INTERVAL {
        std::thread::sleep(POINTER_RECOMPOSE_MIN_INTERVAL - elapsed);
      }
    }
    if self.compose.is_some() && !self.shape_needs_cpu() {
      let base = self.clean.clone();
      match self.compose_frame_gpu(&base, &snap) {
        Ok(()) => {
          self.last_composited = Some(snap);
          self.last_compose_at = Some(Instant::now());
          return Ok(CaptureFrame::NewDesktop {
            texture: self.proc.clone(),
            cursor: None,
          });
        }
        Err(e) => {
          warn(&format!("GPU 光标合成失败，本会话回退 CPU 回读路径: {e}"));
          self.compose = None;
        }
      }
    }
    // unsafe 依据（块级）：CopyResource 为 D3D11 FFI；proc/clean 同设备同尺寸同格式。
    unsafe { self.ctx.CopyResource(&self.proc, &self.clean) };
    let cursor = self.build_cursor_patch(&snap)?;
    self.last_composited = Some(snap);
    self.last_compose_at = Some(Instant::now());
    Ok(CaptureFrame::NewDesktop {
      texture: self.proc.clone(),
      cursor,
    })
  }

  /// GPU 合成一帧（P0-2b）：算光标几何（越界钳制）→ 双流 Blt → proc。
  /// 失败由调用方按「本会话回退 CPU」收口（一次性，不重试）。
  fn compose_frame_gpu(
    &mut self,
    source: &ID3D11Texture2D,
    pointer: &PointerSnapshot,
  ) -> AxResult<()> {
    let cursor = self.compute_cursor_rect(pointer);
    let Some(pipeline) = self.compose.as_mut() else {
      return Err(AxError::internal("GPU 合成管线缺失"));
    };
    pipeline.compose(&self.device, source, cursor.as_ref())
  }

  /// 光标覆盖层的目标矩形 + 形状源偏移（CPU/GPU 两路共用的几何计算）：
  /// 屏幕光标点 − output 原点 − 形状 hotspot，越界钳制到纹理内并同步裁掉源偏移。
  fn compute_cursor_rect(&self, pointer: &PointerSnapshot) -> Option<CursorRect> {
    if !pointer.visible {
      return None;
    }
    let shape = self.shape.as_ref()?;
    // 栅格在 install_shape（形状变更时）已算好；本形状渲染失败 → 跳过覆盖层。
    let raster = shape.raster.as_ref()?;
    if raster.w == 0 || raster.h == 0 {
      return None;
    }
    let hot = shape.info.HotSpot;
    let x0 = pointer.x - self.desktop_rect.0 - hot.x;
    let y0 = pointer.y - self.desktop_rect.1 - hot.y;
    let (tw, th) = (self.desktop_rect.2, self.desktop_rect.3);
    let ix0 = x0.max(0);
    let iy0 = y0.max(0);
    let ix1 = (x0 + raster.w as i32).min(tw);
    let iy1 = (y0 + raster.h as i32).min(th);
    if ix1 <= ix0 || iy1 <= iy0 {
      return None;
    }
    Some(CursorRect {
      x0: ix0,
      y0: iy0,
      w: (ix1 - ix0) as u32,
      h: (iy1 - iy0) as u32,
      sx: (ix0 - x0) as u32,
      sy: (iy0 - y0) as u32,
    })
  }

  /// 拉取/刷新光标形状缓存（要点 5：`PointerShapeBufferSize` 变化触发）。
  ///
  /// `buffer_size == 0` 语义（MS DesktopDuplication 样例口径）：形状**自上次以来
  /// 未变化**，保留缓存——不可清缓存（清掉会让"尺寸未变"的帧整体丢光标）；
  /// 可见性由 `PointerPosition.Visible` 单独表达。
  fn refresh_shape(&mut self, dup: &IDXGIOutputDuplication, buffer_size: u32) -> AxResult<()> {
    if buffer_size == 0 {
      return Ok(());
    }
    // DUPL 投递过形状 = 权威源确认：此后 user32 兜底不再启用（即使同尺寸形状
    // 命中缓存提前返回，也必须先记下这个事实）。
    self.dupl_shape_seen = true;
    if matches!(&self.shape, Some(s) if s.buffer_size == buffer_size) {
      return Ok(()); // 纯移动：位置在 PointerPosition 里，形状不变
    }
    let mut buf = vec![0u8; buffer_size as usize];
    let mut shape_info = DXGI_OUTDUPL_POINTER_SHAPE_INFO::default();
    let mut required = 0u32;
    // unsafe 依据（块级）：GetFramePointerShape 为 DXGI COM FFI；缓冲长度取自
    // 同帧 info.PointerShapeBufferSize（DXGI 自己给出的所需长度），不越界。
    unsafe {
      dup.GetFramePointerShape(
        buffer_size,
        buf.as_mut_ptr() as *mut c_void,
        &mut required,
        &mut shape_info,
      )
      .map_err(|e| AxError::internal(format!("GetFramePointerShape 失败: {e}")))?
    };
    if required > 0 && (required as usize) <= buf.len() {
      buf.truncate(required as usize);
    }
    // 渲染栅格在形状变更时一次算好并缓存（原先每帧重算 + 两块 Vec 分配）。
    // 渲染失败降级为"该形状不打补丁"并 warn——形状是易变输入，
    // 不能让它决定管线生死。
    let raster = match render_cursor_shape(
      shape_info.Type,
      shape_info.Width,
      shape_info.Height,
      shape_info.Pitch,
      &buf,
    ) {
      Ok(r) => Some(r),
      Err(e) => {
        warn(&format!("光标形状渲染失败，本形状周期内跳过光标补丁: {e}"));
        None
      }
    };
    self.install_shape(shape_info, buf, buffer_size, raster);
    Ok(())
  }

  /// 形状落位（DUPL/user32 两源共用的唯一出口）：写形状缓存 + 同步 GPU 覆盖层。
  /// 覆盖层创建失败 → 整会话回退 CPU 回读路径（P0-2b 语义，一次性）。
  fn install_shape(
    &mut self,
    info: DXGI_OUTDUPL_POINTER_SHAPE_INFO,
    data: Vec<u8>,
    buffer_size: u32,
    raster: Option<CursorShapeRaster>,
  ) {
    // GPU 覆盖层（P0-2b）：形状变更（低频）时重建；失败即整会话回退 CPU 回读路径。
    if let Some(pipeline) = self.compose.as_mut() {
      let update = match raster.as_ref() {
        Some(r) => pipeline.update_cursor(&self.device, &self.ctx, r),
        None => {
          pipeline.clear_cursor();
          Ok(())
        }
      };
      if let Err(e) = update {
        warn(&format!("光标覆盖层更新失败，本会话回退 CPU 回读路径: {e}"));
        self.compose = None;
      }
    }
    self.shape = Some(ShapeCache { info, _data: data, buffer_size, raster });
  }

  /// 计算并落盘光标补丁（取缓存栅格 → 回读背景 → `blend_cursor_patch` 混合 → 写回 proc）。
  /// 无可见光标 / 补丁完全越界 / 形状未知（或该形状渲染失败）→ `Ok(None)`
  /// （不打补丁，不视为错误）。混合分支（XOR/alpha/替换/背景）在纯函数里，单测锁定。
  fn build_cursor_patch(&self, pointer: &PointerSnapshot) -> AxResult<Option<CursorPatch>> {
    if !pointer.visible {
      return Ok(None);
    }
    let Some(shape) = self.shape.as_ref() else {
      return Ok(None); // 尚未拿到形状（首次移动前）——宁可不画也不画错
    };
    // 栅格在 install_shape（形状变更时）已算好；本形状渲染失败 → 跳过补丁。
    let Some(raster) = shape.raster.as_ref() else {
      return Ok(None);
    };
    if raster.w == 0
      || raster.h == 0
      || (raster.visible.iter().all(|v| *v == 0) && raster.xor.iter().all(|v| *v == 0))
    {
      return Ok(None);
    }
    // 坐标 = 屏幕光标点 − output DesktopCoordinates.xy − 形状 hotspot（简报要点 4）。
    let hot = shape.info.HotSpot;
    let x0 = pointer.x - self.desktop_rect.0 - hot.x;
    let y0 = pointer.y - self.desktop_rect.1 - hot.y;
    let (tw, th) = (self.desktop_rect.2, self.desktop_rect.3);
    // 越界钳制到纹理内（简报要点 4「越界钳制」），并同步裁掉形状源偏移。
    let ix0 = x0.max(0);
    let iy0 = y0.max(0);
    let ix1 = (x0 + raster.w as i32).min(tw);
    let iy1 = (y0 + raster.h as i32).min(th);
    if ix1 <= ix0 || iy1 <= iy0 {
      return Ok(None);
    }
    let (rw, rh) = ((ix1 - ix0) as u32, (iy1 - iy0) as u32);
    let (sx, sy) = ((ix0 - x0) as u32, (iy0 - y0) as u32);
    // ① 回读补丁矩形下的桌面背景（源 = clean：不含上次烧入的光标；
    // staging 整面创建，CopySubresourceRegion 只搬小块）。
    let b = D3D11_BOX {
      left: ix0 as u32,
      top: iy0 as u32,
      front: 0,
      right: ix1 as u32,
      bottom: iy1 as u32,
      back: 1,
    };
    // unsafe 依据（块级）：CopySubresourceRegion 为 D3D11 FFI；源盒已钳制在
    // clean 尺寸内（ix/iy 的 clamp 在上方）、目标 (0,0) 在 staging 内。
    unsafe { self.ctx.CopySubresourceRegion(&self.bg_staging, 0, 0, 0, 0, &self.clean, 0, Some(&b)) };
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    // unsafe 依据（块级）：Map 为 D3D11 FFI；staging 建时带 CPU_ACCESS_READ。
    // Map 成功后必须 Unmap——下方无条件 Unmap 由闭包收拢保证。
    let map_res: WResult<()> =
      unsafe { self.ctx.Map(&self.bg_staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)) };
    map_res.map_err(|e| AxError::internal(format!("光标背景 Map 失败: {e}")))?;
    // Map 成功后必须 Unmap（后续任何提前返回都不允许跳过）。
    let blend = (|| -> AxResult<Vec<u8>> {
      if mapped.pData.is_null() {
        return Err(AxError::internal("光标背景映射为空"));
      }
      let row_pitch = mapped.RowPitch as usize;
      // ② 背景按补丁宽度压紧（staging 行距可能 > 宽*4；unsafe 切片均在 staging 内：
      // 源盒钳制保证 rw*rh ≤ 纹理尺寸、row_pitch ≥ 每行有效字节）→ 纯函数混合。
      let mut bg = Vec::with_capacity((rw * rh * 4) as usize);
      for r in 0..rh as usize {
        // unsafe 依据（块级）：pData 来自仍持有的 Map（Unmap 在闭包之后执行）。
        let row = unsafe {
          std::slice::from_raw_parts((mapped.pData as *const u8).add(r * row_pitch), (rw * 4) as usize)
        };
        bg.extend_from_slice(row);
      }
      Ok(blend_cursor_patch(raster, sx as usize, sy as usize, rw as usize, rh as usize, &bg))
    })();
    // unsafe 依据（块级）：Unmap 为 D3D11 FFI；无论混合成败都必须与上方 Map 配对。
    unsafe { self.ctx.Unmap(&self.bg_staging, 0) };
    let rgba = blend?;
    // ③ 回写混合结果（矩形已钳制在纹理内）。
    let dst = D3D11_BOX {
      left: ix0 as u32,
      top: iy0 as u32,
      front: 0,
      right: ix1 as u32,
      bottom: iy1 as u32,
      back: 1,
    };
    // unsafe 依据（块级）：UpdateSubresource 为 D3D11 FFI；目标盒已钳制在 proc
    // 尺寸内、srcrowpitch = rw*4 与补丁缓冲逐行等长。
    unsafe {
      self.ctx.UpdateSubresource(
        &self.proc,
        0,
        Some(&dst),
        rgba.as_ptr() as *const c_void,
        rw * 4,
        0,
      )
    };
    Ok(Some(CursorPatch { x: ix0, y: iy0, w: rw, h: rh, rgba }))
  }
}

/// 契约测试缝（`tests/rd_media.rs`）：创建 D3D11 设备（HARDWARE→WARP 回退），
/// 且要求设备实现 `ID3D11VideoDevice`（编码链的 Video Processor 能力）。
/// 用途 = 验证硬件 H.264 编码器配置（异步 MFT 解锁回归）；无可用设备或设备无 VP
/// 能力（基础显示适配器/纯软渲染环境）时返回 None，测试按环境能力缺失跳过而非失败。
pub fn create_device_for_tests() -> Option<ID3D11Device> {
  let (device, _) = create_device().ok()?;
  device.cast::<ID3D11VideoDevice>().ok()?;
  Some(device)
}

/// 契约测试缝（GPU 光标合成的驱动验证，P0-2b）：仅硬件 D3D11 设备 + context，
/// 且要求设备实现 `ID3D11VideoDevice`（video processor 能力）；任一缺失返回 None——
/// 测试按环境能力缺失跳过而非失败：
/// - 无硬件设备（CI 软渲染）：HARDWARE 创建失败；
/// - 基础显示适配器（GitHub runner 一类）：设备可建但 **不实现** `ID3D11VideoDevice`
///   （实测 `No such interface supported`）——运行时对此有整会话 CPU 回退，不是产品失败。
///
/// 不走 WARP 回退（WARP 不代表真实驱动行为）。
pub fn create_hardware_device_for_tests() -> Option<(ID3D11Device, ID3D11DeviceContext)> {
  let (device, ctx) = create_device_with_driver(D3D_DRIVER_TYPE_HARDWARE).ok()?;
  device.cast::<ID3D11VideoDevice>().ok()?;
  Some((device, ctx))
}

/// 要点 1：D3D11CreateDevice（HARDWARE、level ≥ 11_0）失败回退 WARP。
fn create_device() -> AxResult<(ID3D11Device, ID3D11DeviceContext)> {
  for driver in [D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP] {
    match create_device_with_driver(driver) {
      Ok(pair) => return Ok(pair),
      Err(_) => continue,
    }
  }
  // 能力面缺失（spec 失败表：DUPL/D3D 创建失败 → unsupported）。
  Err(AxError::new("unimplemented", "D3D11CreateDevice（HARDWARE/WARP）均失败"))
}

/// 按指定 driver 建 D3D11 设备（feature level ≥ 11_0）。
fn create_device_with_driver(
  driver: windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE,
) -> AxResult<(ID3D11Device, ID3D11DeviceContext)> {
  let levels = [D3D_FEATURE_LEVEL_11_0];
  let mut device: Option<ID3D11Device> = None;
  let mut level = D3D_FEATURE_LEVEL_11_0;
  let mut ctx: Option<ID3D11DeviceContext> = None;
  // unsafe 依据（块级）：D3D11CreateDevice 为 FFI；出参槽位由本栈帧持有。
  let r = unsafe {
    D3D11CreateDevice(
      None,
      driver,
      None,
      D3D11_CREATE_DEVICE_FLAG(0),
      Some(&levels),
      D3D11_SDK_VERSION,
      Some(&mut device),
      Some(&mut level),
      Some(&mut ctx),
    )
  };
  match (r, device, ctx) {
    (Ok(()), Some(d), Some(c)) => Ok((d, c)),
    _ => Err(AxError::new(
      "unimplemented",
      format!("D3D11CreateDevice(driver={}) 失败", driver.0),
    )),
  }
}

/// 要点 2：CreateDXGIFactory1 → EnumAdapters1 → EnumOutputs → 比对
/// `DXGI_OUTPUT_DESC.DesktopCoordinates` → `IDXGIOutput1::DuplicateOutput`。
unsafe fn open_duplication(device: &ID3D11Device, rect: (i32, i32, i32, i32)) -> AxResult<IDXGIOutputDuplication> {
  let factory: IDXGIFactory1 = CreateDXGIFactory1()
    .map_err(|e| AxError::internal(format!("CreateDXGIFactory1 失败: {e}")))?;
  let mut matched: Option<IDXGIOutput> = None;
  'adapters: for ai in 0..u32::MAX {
    // unsafe 依据（块级）：EnumAdapters1 为 DXGI COM FFI；单参返回 Result 形态，
    // Err(DXGI_ERROR_NOT_FOUND) 即枚举尽，无出参槽位。
    let adapter = match unsafe { factory.EnumAdapters1(ai) } {
      Ok(a) => a,
      Err(_) => break,
    };
    for oi in 0..u32::MAX {
      // unsafe 依据（块级）：EnumOutputs 为 DXGI COM FFI；返回值形态（单参 →
      // Result），Err 即枚举尽，无出参槽位。
      let output = match unsafe { adapter.EnumOutputs(oi) } {
        Ok(o) => o,
        Err(_) => break,
      };
      let desc: DXGI_OUTPUT_DESC = match output.GetDesc() {
        Ok(d) => d,
        Err(_) => continue,
      };
      if desc.AttachedToDesktop == BOOL(0) {
        continue;
      }
      let r = desc.DesktopCoordinates;
      if (r.left, r.top, r.right - r.left, r.bottom - r.top) == rect {
        matched = Some(output);
        break 'adapters;
      }
    }
  }
  let output = matched.ok_or_else(|| {
    AxError::new("invalid_request", format!("未找到 DesktopCoordinates 匹配的输出: {rect:?}"))
  })?;
  let output1 = output
    .cast::<IDXGIOutput1>()
    .map_err(|e| AxError::internal(format!("IDXGIOutput1 QI 失败: {e}")))?;
  // 失败语义（简报 Step 4）：DuplicateOutput 失败 = 能力面缺失 → unimplemented。
  output1
    .DuplicateOutput(device)
    .map_err(|e| AxError::new("unimplemented", format!("DuplicateOutput 失败（无桌面会话/已被占用）: {e}")))
}

/// 建 2D 纹理（bind/usage/cpu 访问按入参），失败 → `internal`。
/// `pub(crate)`：encode.rs 的 NV12/staging 目标复用同一创建逻辑（同 crate 内共享，
/// 不外泄到 crate 外）。
pub(crate) fn create_texture(
  device: &ID3D11Device,
  w: u32,
  h: u32,
  format: DXGI_FORMAT,
  usage: D3D11_USAGE,
  cpu_access: D3D11_CPU_ACCESS_FLAG,
  bind: D3D11_BIND_FLAG,
) -> AxResult<ID3D11Texture2D> {
  let desc = D3D11_TEXTURE2D_DESC {
    Width: w,
    Height: h,
    MipLevels: 1,
    ArraySize: 1,
    Format: format,
    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
    Usage: usage,
    BindFlags: bind.0 as u32,
    CPUAccessFlags: cpu_access.0 as u32,
    MiscFlags: 0,
  };
  let mut tex: Option<ID3D11Texture2D> = None;
  // unsafe 依据（块级）：CreateTexture2D 为 D3D11 FFI；desc 由本函数构造、
  // 出参槽位在本栈帧，返回的纹理引用由调用方持有。
  unsafe {
    device
      .CreateTexture2D(&desc, None, Some(&mut tex))
      .map_err(|e| AxError::internal(format!("CreateTexture2D({w}x{h}) 失败: {e}")))?
  };
  tex.ok_or_else(|| AxError::internal("CreateTexture2D 未返回纹理"))
}

/// P0-2b 驱动验证与正确性回归（真机 GPU 上执行；CI 无硬件设备跳过）。
///
/// 这是「GPU 光标合成」的驱动行为验证：在真实硬件设备上构造底图（纯红）+
/// 覆盖层（左半不透明绿 / 右半全透明 / 一格半透明绿），经双流 VideoProcessor
/// 合成到目标纹理后回读断言：
/// - 不透明像素被覆盖、覆盖矩形外不受影响；
/// - 全透明像素保留背景（证明用的是 per-pixel alpha，而不是会被误绘的恒定 alpha）；
/// - 半透明像素按 α 混合（证明真混合而非直写）。
///
/// 跳过门槛（环境能力缺失，不算产品失败）：无硬件设备（CI 软渲染）或设备不实现
/// `ID3D11VideoDevice`（基础显示适配器）——后者正是运行时 CPU 回退覆盖的情形。
/// 管线创建失败在无 GPU 的 CI 上不会发生（设备缺失提前跳过）；在**具备 VP 能力的**
/// 真实硬件上失败即「本驱动不支持 P0-2b 机制」——运行时对此有整会话 CPU 回退，不会带病运行。
#[cfg(test)]
mod gpu_cursor_tests {
  use super::*;

  fn read_pixel(
    device: &ID3D11Device,
    ctx: &ID3D11DeviceContext,
    texture: &ID3D11Texture2D,
    w: u32,
    h: u32,
    x: u32,
    y: u32,
  ) -> [u8; 4] {
    let staging = create_texture(
      device,
      w,
      h,
      DXGI_FORMAT_B8G8R8A8_UNORM,
      D3D11_USAGE_STAGING,
      D3D11_CPU_ACCESS_READ,
      D3D11_BIND_FLAG(0),
    )
    .expect("staging 创建");
    // unsafe 依据（块级）：CopyResource/Map/Unmap 为 D3D11 FFI；staging 为本函数持有。
    unsafe { ctx.CopyResource(&staging, texture) };
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    unsafe { ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)) }.expect("Map");
    let row_pitch = mapped.RowPitch as usize;
    let mut px = [0u8; 4];
    // unsafe 依据（块级）：pData 来自仍持有的 Map；xy 在纹理尺寸内。
    unsafe {
      let p = (mapped.pData as *const u8).add(y as usize * row_pitch + x as usize * 4);
      px.copy_from_slice(std::slice::from_raw_parts(p, 4));
    }
    unsafe { ctx.Unmap(&staging, 0) };
    px
  }

  #[test]
  fn gpu_cursor_overlay_composites_with_per_pixel_alpha() {
    let Some((device, ctx)) = create_hardware_device_for_tests() else {
      return; // 无硬件设备（CI 软渲染）：按环境能力缺失跳过
    };
    let (w, h) = (64u32, 64u32);
    // 底图：纯红（BGRA = 0,0,255,255）。
    let base = create_texture(
      &device,
      w,
      h,
      DXGI_FORMAT_B8G8R8A8_UNORM,
      D3D11_USAGE_DEFAULT,
      D3D11_CPU_ACCESS_FLAG(0),
      D3D11_BIND_FLAG(0),
    )
    .expect("底图创建");
    let red_pixels: Vec<u8> = [0u8, 0u8, 255u8, 255u8]
      .iter()
      .copied()
      .cycle()
      .take((w * h * 4) as usize)
      .collect();
    // unsafe 依据（块级）：UpdateSubresource 为 D3D11 FFI；缓冲逐行 w*4 与纹理等宽。
    unsafe {
      ctx.UpdateSubresource(&base, 0, None, red_pixels.as_ptr() as *const c_void, w * 4, 0)
    };
    // 合成目标。
    let target = create_texture(
      &device,
      w,
      h,
      DXGI_FORMAT_B8G8R8A8_UNORM,
      D3D11_USAGE_DEFAULT,
      D3D11_CPU_ACCESS_FLAG(0),
      D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE,
    )
    .expect("目标纹理创建");
    let mut pipeline = ComposePipeline::create(&device, &ctx, &target, w, h)
      .expect("GPU 光标合成管线创建（失败 = 本驱动不支持 P0-2b 机制）");
    // 覆盖层 8×8：列 0..3 不透明绿；列 4..7 全透明（颜色故意给蓝——若被恒定 alpha
    // 误用会整块绘出）；像素 (7,0) 半透明绿（alpha=128）。
    let mut bgra = vec![0u8; 8 * 8 * 4];
    let mut visible = vec![0u8; 8 * 8];
    for y in 0..8usize {
      for x in 0..8usize {
        let i = (y * 8 + x) * 4;
        if x < 4 {
          bgra[i..i + 4].copy_from_slice(&[0, 255, 0, 255]); // 绿
          visible[y * 8 + x] = 1;
        } else {
          bgra[i..i + 4].copy_from_slice(&[255, 0, 0, 255]); // 蓝（不可见，不应被绘制）
        }
      }
    }
    bgra[28..32].copy_from_slice(&[0, 255, 0, 128]); // 像素 (7,0)：半透明绿
    visible[7] = 1;
    let raster = CursorShapeRaster { bgra, visible, xor: vec![0u8; 64], w: 8, h: 8 };
    pipeline.update_cursor(&device, &ctx, &raster).expect("覆盖层纹理");

    // 合成：覆盖矩形贴在 (16,16)。
    let rect = CursorRect { x0: 16, y0: 16, w: 8, h: 8, sx: 0, sy: 0 };
    pipeline.compose(&device, &base, Some(&rect)).expect("合成 Blt");

    let green = read_pixel(&device, &ctx, &target, w, h, 17, 17);
    assert_eq!(green, [0, 255, 0, 255], "不透明像素应被覆盖为纯绿: {green:?}");
    let preserved = read_pixel(&device, &ctx, &target, w, h, 21, 17);
    assert_eq!(
      preserved,
      [0, 0, 255, 255],
      "全透明像素必须保留背景红（per-pixel alpha，而非恒定 alpha）: {preserved:?}"
    );
    let blended = read_pixel(&device, &ctx, &target, w, h, 23, 16);
    // 半透明绿（0,255,0,α=0.5）叠纯红（B=0,G=0,R=255）→ B=0, G≈128, R≈127。
    // alpha 通道本身（191）不参与下游语义（proc 后续转 NV12，不消费 alpha），不断言。
    assert!(
      blended[0] <= 4
        && (blended[1] as i32 - 128).abs() <= 4
        && (blended[2] as i32 - 127).abs() <= 4,
      "半透明像素应按 α 混合（B≈0, G≈128, R≈127）: {blended:?}"
    );
    let outside = read_pixel(&device, &ctx, &target, w, h, 8, 8);
    assert_eq!(outside, [0, 0, 255, 255], "覆盖矩形外不受影响");
  }

  #[test]
  fn gpu_cursor_overlay_builder_marks_transparent_pixels() {
    // 纯函数：可见=原色（含 alpha），不可见=全透明——上层混合依赖该不变量。
    let raster = CursorShapeRaster {
      bgra: vec![9, 8, 7, 255, 1, 2, 3, 255],
      visible: vec![1, 0],
      xor: vec![0, 0],
      w: 2,
      h: 1,
    };
    assert_eq!(build_cursor_overlay(&raster), vec![9, 8, 7, 255, 0, 0, 0, 0]);
  }
}

/// CPU 补丁混合的纯逻辑回归（跨平台，无 Win32/硬件依赖；spec 测试策略「光标语义」）。
/// 锁四个分支：XOR（bg^color，MASKED_COLOR 0xFF 掩码）/ 真 alpha 混合 / 替换 / 保留背景。
#[cfg(test)]
mod cursor_blend_tests {
  use super::*;

  /// 构造 2×1 栅格：p0 = (x0,0) 分支位由入参决定，p1 = 不可见背景像素。
  fn raster_2x1(bgra0: [u8; 4], vis0: u8, xor0: u8) -> CursorShapeRaster {
    CursorShapeRaster {
      w: 2,
      h: 1,
      bgra: [bgra0, [0, 0, 0, 0]].concat(),
      visible: vec![vis0, 0],
      xor: vec![xor0, 0],
    }
  }

  #[test]
  fn blend_replaces_opaque_pixel_and_keeps_background() {
    // a=255 → 直接替换；第二像素不可见 → 背景原样。
    let raster = raster_2x1([10, 20, 30, 255], 1, 0);
    let out = blend_cursor_patch(&raster, 0, 0, 2, 1, &[1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(out, vec![10, 20, 30, 255, 5, 6, 7, 8]);
  }

  #[test]
  fn blend_blends_straight_alpha_over_background() {
    // a=128、颜色 (0,0,255)（纯红域）叠背景 (0,0,0)（黑）→ R ≈ 128（直通 alpha）。
    let raster = raster_2x1([0, 0, 255, 128], 1, 0);
    let out = blend_cursor_patch(&raster, 0, 0, 2, 1, &[0, 0, 0, 0, 9, 9, 9, 9]);
    let px = &out[0..4];
    assert_eq!(px[0], 0, "B 不变");
    assert_eq!(px[1], 0, "G 不变");
    assert!((px[2] as i32 - 128).abs() <= 1, "R ≈ 128: {px:?}");
    assert_eq!(px[3], 255, "输出 alpha 恒 255（NV12 不消费）");
    assert_eq!(&out[4..8], &[9, 9, 9, 9], "背景像素原样");
  }

  #[test]
  fn blend_alpha_zero_keeps_background() {
    // 可见但 a=0（COLOR 全透明防御分支）→ 保留背景。
    let raster = raster_2x1([7, 7, 7, 0], 1, 0);
    let out = blend_cursor_patch(&raster, 0, 0, 2, 1, &[1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(out, vec![1, 2, 3, 4, 5, 6, 7, 8]);
  }

  #[test]
  fn blend_xors_masked_color_over_background() {
    // XOR 分支（MASKED_COLOR 0xFF 掩码）：bg ^ color；bg ^ 0 = 保留背景（掩码恒等）。
    let raster = raster_2x1([0xFF, 0x0F, 0x00, 255], 1, 1);
    let out = blend_cursor_patch(&raster, 0, 0, 2, 1, &[0x00, 0xF0, 0x00, 0, 5, 6, 7, 8]);
    assert_eq!(&out[0..4], &[0xFF, 0xFF, 0x00, 255], "bg ^ color");
    assert_eq!(&out[4..8], &[5, 6, 7, 8], "背景像素原样");
    let same = raster_2x1([0, 0, 0, 255], 1, 1);
    let keep = blend_cursor_patch(&same, 0, 0, 2, 1, &[1, 2, 3, 4, 5, 6, 7, 8]);
    // XOR 恒等只保 RGB；输出 alpha 按契约恒 255（NV12 不消费，与其余分支一致）。
    assert_eq!(&keep[0..3], &[1, 2, 3], "XOR 0 恒等保留背景 RGB");
    assert_eq!(keep[3], 255, "XOR 输出 alpha 恒 255");
    assert_eq!(&keep[4..8], &[5, 6, 7, 8], "背景像素原样");
  }

  #[test]
  fn blend_honors_source_offset_clipping() {
    // 源偏移 (sx=1)：栅格第 0 列被裁掉，只取第 1 列像素落补丁第 0 位。
    let raster = CursorShapeRaster {
      w: 2,
      h: 1,
      bgra: [9, 9, 9, 255, 42, 0, 0, 255].to_vec(),
      visible: vec![1, 1],
      xor: vec![0, 0],
    };
    let out = blend_cursor_patch(&raster, 1, 0, 1, 1, &[0, 0, 0, 0]);
    assert_eq!(out, vec![42, 0, 0, 255]);
  }

  #[test]
  fn render_color_with_mask_honors_and_mask() {
    // user32 兜底的经典颜色光标（无 alpha，透明性由 AND 掩码表达）：
    // w=16 → 颜色行距 64、掩码行距 4（DIB 1bpp 恒 4 字节对齐）。
    let bgra = vec![0u8; 16 * 4]; // 16×1 全黑颜色区
    let mut mask = [0u8; 4];
    mask[0] = 0b1111_0000; // x=0..3 AND=1 → 透明；x=4..15 AND=0 → 可见
    let r = render_color_with_mask(16, 1, 64, &bgra, &mask, 4).expect("掩码渲染");
    assert_eq!(&r.visible[0..4], &[0, 0, 0, 0], "AND=1 透明");
    assert_eq!(&r.visible[4..8], &[1, 1, 1, 1], "AND=0 可见");
    assert_eq!(&r.bgra[0..4], &[0, 0, 0, 255], "颜色按不透明存储（alpha 归 255）");
    assert!(r.xor.iter().all(|v| *v == 0), "经典颜色光标无 XOR");
    // 截断缓冲 → 错误。
    assert!(render_color_with_mask(16, 1, 64, &bgra[..63], &mask, 4).is_err());
  }
}

/// 实机回归（软光标/虚拟显示环境，2026-10-13 实证：IDD 虚拟屏 + 远控接管显示下
/// DUPL 零指针投递 → 旧实现光标永不合成）：DUPL 零投递时 user32 兜底必须装出
/// 形状栅格。环境能力缺失（打不开 duplication / 观察窗内无桌面帧 / DUPL 正常投递）
/// → 跳过，不算失败——断言只在「零投递且取到帧」的环境成立。
#[cfg(test)]
mod user32_fallback_live_tests {
  use super::*;
  use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetSystemMetrics, SetCursorPos, SM_CXSCREEN, SM_CYSCREEN,
  };

  #[test]
  fn user32_fallback_installs_shape_when_dupl_silent() {
    let _com = ComScope::enter();
    let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    let Ok(mut out) = DuplicationOutput::open((0, 0, sw, sh)) else {
      return; // 无桌面会话（CI 服务态）：能力缺失跳过
    };
    let deadline = Instant::now() + Duration::from_secs(12);
    let mut saw_desktop_frame = false;
    let mut i = 0i32;
    while Instant::now() < deadline {
      i += 1;
      // 轻微晃动（±1px）制造桌面更新——DXGI 变化驱动，静止桌面不出帧。
      let mut pt = windows::Win32::Foundation::POINT::default();
      if unsafe { GetCursorPos(&mut pt) }.is_ok() {
        let _ = unsafe { SetCursorPos(pt.x + (i % 2), pt.y + ((i / 2) % 2)) };
      }
      match out.next_frame(300) {
        Ok(CaptureFrame::NewDesktop { .. }) => {
          saw_desktop_frame = true;
          break;
        }
        Ok(_) => {}
        Err(_) => return, // ACCESS_LOST 等：环境能力缺失，跳过
      }
    }
    if !saw_desktop_frame {
      return; // 观察窗内桌面静止 → resolve 未执行，跳过
    }
    if out.dupl_shape_seen {
      return; // DUPL 正常投递的环境走权威路径，不属本用例
    }
    // 环境能力缺失跳过（CI 托管 runner 无可见光标即此形态——2026-10-13 发版失败实证：
    // GetCursorInfo 不报 showing → 兜底只回隐藏快照、不建形状缓存；或 GetIconInfo 失败
    // 缓存 key=0）。断言只在「真有可见光标且取形成功」的实机成立，真机回归语义不变。
    let Some(cache) = out.user32_shape.as_ref() else {
      return;
    };
    if cache.key == 0 {
      return;
    }
    let shape = out
      .shape
      .as_ref()
      .expect("user32 兜底命中（key≠0）必有形状缓存");
    let raster = shape
      .raster
      .as_ref()
      .expect("key≠0 时栅格随 install_shape 落位（光标不可见根因回归）");
    assert!(raster.w > 0 && raster.h > 0, "栅格尺寸非空: {}x{}", raster.w, raster.h);
    assert!(
      raster.visible.contains(&1),
      "可见形状不得整块透明（alpha 语义回归）"
    );
    eprintln!(
      "[probe] user32 fallback OK: shape {}x{}, visible={}",
      raster.w,
      raster.h,
      raster.visible.iter().filter(|v| **v == 1).count()
    );
    // 光标运动补帧回归（2026-10-13「连上但卡死」根因）：零投递环境没有 DUPL 指针
    // 事件唤醒合成——动鼠标必须由轮询补帧路径自己产帧，否则画面只有桌面像素恰好
    // 变化时才刷新。持续小挪光标 + 期限内必须见到 NewDesktop（桌面偶发重绘产帧同样
    // 放行：合同是「有运动就有帧」）。
    let start_pt = {
      let mut pt = windows::Win32::Foundation::POINT::default();
      match unsafe { GetCursorPos(&mut pt) } {
        Ok(()) => pt,
        Err(_) => return, // 取不到位置：环境能力缺失，跳过
      }
    };
    let motion_deadline = Instant::now() + Duration::from_secs(3);
    let mut got_motion_frame = false;
    let mut mi = 0i32;
    while Instant::now() < motion_deadline {
      mi += 1;
      let _ = unsafe { SetCursorPos(start_pt.x + (mi % 3), start_pt.y + ((mi / 3) % 3)) };
      match out.next_frame(100) {
        Ok(CaptureFrame::NewDesktop { .. }) => {
          got_motion_frame = true;
          break;
        }
        Ok(_) => {}
        Err(_) => return,
      }
    }
    assert!(got_motion_frame, "光标运动必须在期限内产出帧（零投递轮询补帧回归）");
    eprintln!("[probe] pointer-motion frames OK ({} 次挪动)", mi);
  }
}
