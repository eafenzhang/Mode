//! DXGI Desktop Duplication 采集（spec「被控 Rust · 采集/光标」）。
//!
//! 所有 Win32/DXGI/D3D11 调用集中在本文件；seq/GOP/冻结状态机仍在 `state.rs`
//! （本文件不碰），保证纯逻辑可 `cargo test`。
//!
//! 光标合成路线（spec：光标烧进画面，`cursor-none` 语义）：DUPL 帧**不含**光标
//! （硬件光标由 DWM 叠加显示），必须自行合成。这里用「CPU 渲染形状 + 小块 GPU 回读
//! 背景 + UpdateSubresource 回写」：
//! - 形状渲染是纯函数 `render_cursor_shape`（单色/颜色/掩码三种 `PointerShapeType`，
//!   无 Win32 依赖，单测覆盖）；
//! - 半透明/掩码光标的透明像素必须保留桌面背景——直接整块 `UpdateSubresource`
//!   覆盖会在光标包围盒内留出色块（spec 验收项「对端光标在画面内随动」不能是色块），
//!   故先把补丁矩形从 proc 纹理回读（`CopySubresourceRegion` 到 staging）再逐像素混合；
//! - spec 的 GPU quad 路线是主案、本实现是等效的 CPU 混合变体；若未来要换 GPU quad，
//!   对外协议与 `CaptureFrame` 形状零改动（spec 降级预案同口径）。
use std::ffi::c_void;

use windows::Win32::Foundation::BOOL;
use windows::Win32::Graphics::Direct3D11::{
  ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_BIND_FLAG,
  D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX,
  D3D11_CPU_ACCESS_FLAG, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_FLAG, D3D11_MAP_READ,
  D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE,
  D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING, D3D11CreateDevice,
};
use windows::Win32::Graphics::Direct3D::{
  D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0,
};
use windows::Win32::Graphics::Dxgi::{
  CreateDXGIFactory1, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_WAIT_TIMEOUT,
  DXGI_OUTDUPL_FRAME_INFO, DXGI_OUTDUPL_POINTER_SHAPE_INFO,
  DXGI_OUTDUPL_POINTER_SHAPE_TYPE_COLOR, DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MASKED_COLOR,
  DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME, DXGI_OUTPUT_DESC, IDXGIOutput, IDXGIOutput1,
  IDXGIOutputDuplication, IDXGIFactory1, IDXGIResource,
};
use windows::Win32::Graphics::Dxgi::Common::{
  DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::core::{Interface, Result as WResult};

use crate::error::{AxError, AxResult};

use super::warn;

/// 光标形状缓存（`GetFramePointerShape` 原始缓冲 + 解析后的形状头）。
struct ShapeCache {
  info: DXGI_OUTDUPL_POINTER_SHAPE_INFO,
  data: Vec<u8>,
  /// 缓存命中的 `PointerShapeBufferSize`（形状变更检测键，简报要点 5）。
  buffer_size: u32,
}

/// 一帧采集结果（纹理是 GPU 句柄，seq 由外层 `state` 分配）。
pub enum CaptureFrame {
  /// 新桌面帧：已含光标补丁的 proc 纹理 + 光标补丁信息（供观测）。
  NewDesktop { texture: ID3D11Texture2D, cursor: Option<CursorPatch> },
  /// 无新帧（AcquireNextFrame 超时/仅元数据变化）。
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

/// 形状渲染结果：BGRA 像素 + 每像素可见性掩码（1=光标实体，0=透明、露出背景）。
pub struct CursorShapeRaster {
  pub w: u32,
  pub h: u32,
  pub bgra: Vec<u8>,
  pub visible: Vec<u8>,
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
  Ok(CursorShapeRaster { w, h, bgra, visible })
}

/// 颜色光标（`PointerShapeType == 2`）：BGRA 位图直接拷贝（alpha 统一 255——
/// 补丁是不透明覆盖，混合语义交给 visible 掩码）。
fn render_color(w: u32, h: u32, pitch: u32, data: &[u8]) -> AxResult<CursorShapeRaster> {
  let need = (pitch as usize).checked_mul(h as usize)
    .ok_or_else(|| AxError::internal("光标形状尺寸溢出"))?;
  if data.len() < need {
    return Err(AxError::internal("光标形状缓冲不足（颜色）"));
  }
  let mut bgra = vec![0u8; (w * h * 4) as usize];
  for y in 0..h as usize {
    let row = &data[y * pitch as usize..];
    for x in 0..w as usize {
      let src = &row[x * 4..x * 4 + 4];
      let i = (y * w as usize + x) * 4;
      bgra[i..i + 4].copy_from_slice(&[src[0], src[1], src[2], 255]);
    }
  }
  let visible = vec![1u8; (w * h) as usize];
  Ok(CursorShapeRaster { w, h, bgra, visible })
}

/// 掩码颜色光标（`PointerShapeType == 4`）：前部 BGRA 颜色（pitch 行距）+ 尾部
/// 1bpp AND 掩码（行距按 4 字节对齐）。AND=1 → 透明。
fn render_masked_color(
  w: u32,
  h: u32,
  pitch: u32,
  data: &[u8],
) -> AxResult<CursorShapeRaster> {
  let color_len = (pitch as usize).checked_mul(h as usize)
    .ok_or_else(|| AxError::internal("光标形状尺寸溢出"))?;
  let mask_stride = (w as usize).div_ceil(32) * 4;
  let need = color_len + mask_stride * h as usize;
  if data.len() < need {
    return Err(AxError::internal("光标形状缓冲不足（掩码颜色）"));
  }
  let mut bgra = vec![0u8; (w * h * 4) as usize];
  let mut visible = vec![0u8; (w * h) as usize];
  for y in 0..h as usize {
    let row = &data[y * pitch as usize..];
    let and_row = &data[color_len + y * mask_stride..];
    for x in 0..w as usize {
      let bit = 7 - (x % 8);
      if (and_row[x / 8] >> bit) & 1 == 1 {
        continue; // AND=1 → 透明
      }
      let src = &row[x * 4..x * 4 + 4];
      let i = (y * w as usize + x) * 4;
      bgra[i..i + 4].copy_from_slice(&[src[0], src[1], src[2], 255]);
      visible[y * w as usize + x] = 1;
    }
  }
  Ok(CursorShapeRaster { w, h, bgra, visible })
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
    Ok(CursorShapeRaster { w: 0, h: 0, bgra: Vec::new(), visible: Vec::new() })
  }
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

/// 单个输出的 duplication 会话：D3D11 device + proc 纹理 + DXGI 复制。
pub struct DuplicationOutput {
  device: ID3D11Device,
  ctx: ID3D11DeviceContext,
  /// `None` = 已 close（释放 duplication，简报要点 6）。
  duplication: Option<IDXGIOutputDuplication>,
  /// 目标输出的物理矩形 (x, y, w, h)：光标屏幕坐标 → 纹理坐标的换算基准。
  desktop_rect: (i32, i32, i32, i32),
  /// 采集结果落点：DEFAULT/BGRA、尺寸 = 输出桌面（CopyResource 目标 + 光标回写目标）。
  proc: ID3D11Texture2D,
  /// 光标背景回读用 staging（STAGING/CPU_READ，同 desc）。
  bg_staging: ID3D11Texture2D,
  shape: Option<ShapeCache>,
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
    // proc/staging 尺寸取真实输出矩形（与 acquire 到的帧纹理同尺寸、同格式——
    // DUPL 帧恒为 B8G8R8A8_UNORM）。
    let (w, h) = (rect.2 as u32, rect.3 as u32);
    // proc 的 bind 同时带 RENDER_TARGET（语义上可作拷贝目标）与 SHADER_RESOURCE
    // （编码链会为它建 VP 输入视图，视图类型按实现可能要求 SRV 位）——多带无害、
    // 少带会在部分驱动上直接 CreateVideoProcessorInputView 失败。
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
    Ok(Self { device, ctx, duplication: Some(duplication), desktop_rect: rect, proc, bg_staging, shape: None })
  }

  /// 采集端持有的 D3D11 device（编码链 `Encoder::new` 复用同一 device，
  /// 保证 VP 输入视图与 proc 纹理同设备）。
  pub fn device(&self) -> &ID3D11Device { &self.device }

  /// 关闭：释放 duplication（COM 引用随 drop 归零）；幂等。
  pub fn close(&mut self) { self.duplication = None; }

  /// 阻塞至下一帧（`timeout_ms` 典型 16ms）。返回前已把桌面纹理 `CopyResource`
  /// 到 proc 纹理并回写光标补丁（`cursor.rgba` 为 **BGRA** 的已混合字节）。
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
      Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(CaptureFrame::Idle),
      Err(e) => {
        return Err(AxError::internal(format!("AcquireNextFrame 失败: {e}")));
      }
    }
    // 从这里开始所有分支都由守卫保证 Release（含 Idle 提前返回与 `?` 错误路径）。
    let _rel = FrameRelease(&dup);
    // 要点 3a：无新帧（无 present 且无累积帧）→ Idle。
    if info.LastPresentTime == 0 && info.AccumulatedFrames == 0 {
      return Ok(CaptureFrame::Idle);
    }
    // 要点 5：形状缓冲大小变化才重新拉取（纯移动只更新位置）。
    self.refresh_shape(&dup, info.PointerShapeBufferSize)?;
    let res = resource.ok_or_else(|| AxError::internal("AcquireNextFrame 未返回桌面纹理"))?;
    let frame = res.cast::<ID3D11Texture2D>().map_err(|e| {
      AxError::internal(format!("桌面资源转 ID3D11Texture2D 失败: {e}"))
    })?;
    // unsafe 依据（块级）：CopyResource 为 D3D11 FFI；proc/frame 同设备同尺寸
    // （DUPL 帧恒 B8G8R8A8_UNORM 且与输出矩形一致，open 时已对齐）。
    unsafe { self.ctx.CopyResource(&self.proc, &frame) };
    // 光标补丁（内部做背景回读 + 混合 + UpdateSubresource；无可见光标 → None）。
    let cursor = self.build_cursor_patch(&info)?;
    Ok(CaptureFrame::NewDesktop { texture: self.proc.clone(), cursor })
  }

  /// 拉取/刷新光标形状缓存（要点 5：`PointerShapeBufferSize` 变化触发）。
  fn refresh_shape(&mut self, dup: &IDXGIOutputDuplication, buffer_size: u32) -> AxResult<()> {
    if buffer_size == 0 {
      self.shape = None; // 无指针形状（如远程会话暂无光标）
      return Ok(());
    }
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
    self.shape = Some(ShapeCache { info: shape_info, data: buf, buffer_size });
    Ok(())
  }

  /// 计算并落盘光标补丁（渲染形状 → 回读背景 → 混合 → `UpdateSubresource`）。
  /// 无可见光标 / 补丁完全越界 / 形状未知 → `Ok(None)`（不打补丁，不视为错误）。
  fn build_cursor_patch(&self, info: &DXGI_OUTDUPL_FRAME_INFO) -> AxResult<Option<CursorPatch>> {
    let pos = info.PointerPosition;
    if pos.Visible == BOOL(0) {
      return Ok(None);
    }
    let Some(shape) = self.shape.as_ref() else {
      return Ok(None); // 尚未拿到形状（首次移动前）——宁可不画也不画错
    };
    // 光标形状是易变输入（文件头原则：不能让它决定管线生死）——渲染失败只降级
    // 为「本帧不打光标补丁」并 warn；纹理/设备级错误（Map/回读）仍向上冒泡。
    let raster = match render_cursor_shape(
      shape.info.Type,
      shape.info.Width,
      shape.info.Height,
      shape.info.Pitch,
      &shape.data,
    ) {
      Ok(r) => r,
      Err(e) => {
        warn(&format!("光标形状渲染失败，本帧跳过光标补丁: {e}"));
        return Ok(None);
      }
    };
    if raster.w == 0 || raster.h == 0 || raster.visible.iter().all(|v| *v == 0) {
      return Ok(None);
    }
    // 坐标 = 屏幕光标点 − output DesktopCoordinates.xy − 形状 hotspot（简报要点 4）。
    let hot = shape.info.HotSpot;
    let x0 = pos.Position.x - self.desktop_rect.0 - hot.x;
    let y0 = pos.Position.y - self.desktop_rect.1 - hot.y;
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
    // ① 回读补丁矩形下的桌面背景（staging 整面创建，CopySubresourceRegion 只搬小块）。
    let b = D3D11_BOX {
      left: ix0 as u32,
      top: iy0 as u32,
      front: 0,
      right: ix1 as u32,
      bottom: iy1 as u32,
      back: 1,
    };
    // unsafe 依据（块级）：CopySubresourceRegion 为 D3D11 FFI；源盒已钳制在
    // proc 尺寸内（ix/iy 的 clamp 在上方）、目标 (0,0) 在 staging 内。
    unsafe { self.ctx.CopySubresourceRegion(&self.bg_staging, 0, 0, 0, 0, &self.proc, 0, Some(&b)) };
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
      // ② 逐像素混合：可见=光标形状，透明=背景（staging 的行距可能 > 宽*4）。
      let mut patch = Vec::with_capacity((rw * rh * 4) as usize);
      for r in 0..rh as usize {
        // unsafe 依据（块级）：pData 来自仍持有的 Map（Unmap 在闭包之后执行）；
        // 起点 r*rowPitch、长度 rw*4 均在 staging（整面 desktop 尺寸）内——
        // 源盒钳制保证 rw*rh ≤ 纹理尺寸、rowPitch ≥ 每行有效字节。
        let bg_row = unsafe {
          std::slice::from_raw_parts((mapped.pData as *const u8).add(r * row_pitch), (rw * 4) as usize)
        };
        for c in 0..rw as usize {
          let src = ((sy as usize + r) * raster.w as usize + (sx as usize + c)) * 4;
          let vis = raster.visible[(sy as usize + r) * raster.w as usize + (sx as usize + c)];
          if vis == 1 {
            patch.extend_from_slice(&raster.bgra[src..src + 4]);
          } else {
            patch.extend_from_slice(&bg_row[c * 4..c * 4 + 4]);
          }
        }
      }
      Ok(patch)
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

/// 要点 1：D3D11CreateDevice（HARDWARE、level ≥ 11_0）失败回退 WARP。
fn create_device() -> AxResult<(ID3D11Device, ID3D11DeviceContext)> {
  let levels = [D3D_FEATURE_LEVEL_11_0];
  for driver in [D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP] {
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
      (Ok(()), Some(d), Some(c)) => return Ok((d, c)),
      _ => continue,
    }
  }
  // 能力面缺失（spec 失败表：DUPL/D3D 创建失败 → unsupported）。
  Err(AxError::new("unimplemented", "D3D11CreateDevice（HARDWARE/WARP）均失败"))
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
