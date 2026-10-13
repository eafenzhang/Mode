//! 采集纹理 → D3D11 Video Processor（BGRA→NV12 + 缩放）→ MF 硬件 H.264 → Annex-B ES。
//!
//! 链路（spec「被控 Rust · 编码」）：proc 纹理（BGRA，已含光标）→
//! `ID3D11VideoProcessor`（Blt 顺带完成到 maxWidth 的缩放，目标 NV12）→
//! `IMFTransform`（`MFT_CATEGORY_VIDEO_ENCODER` 硬件 H.264）→ Annex-B ES。
//!
//! 输入侧两条路（性能优化）：
//! - 主路：D3D11↔MF 互操作（`MFT_MESSAGE_SET_D3D_MANAGER` + `MFCreateDXGISurfaceBuffer`），
//!   NV12 纹理直接进编码器，零回读零拷贝；
//! - 回退路：建管理器 / 类型协商失败，或运行中 `ProcessInput` 拒绝 DXGI 输入时一次性回退，
//!   即 staging Map 回读紧凑 NV12 再灌内存样本（spec 降级预案）。
//!
//! 两条路的 seq/时间戳/flush-IDR/配对语义完全一致，回退对上层与线上格式零影响。
//!
//! 即时 IDR（Resolution 2 + 后续优化）：windows 0.58 元数据与真实 MF 均**没有**
//! `MF_MT_MPEG2_GOP_SIZE` 属性（H.264 编码 MFT 不暴露 GOP 配置）；状态机（state.rs）
//! 判定本帧应为关键帧时（首帧后的 GOP 到期 / `force_idr` 换屏 / 解冻 / need-key），
//! 首选 `ICodecAPI::SetValue(CODECAPI_AVEncVideoForceKeyFrame, TRUE)`——下一帧编码为
//! IDR，不丢帧、不清 rate-control 状态；输出侧以「期望关键帧的配对项拿到非 IDR」为
//! 失效信号，一旦未生效即永久降级 `MFT_MESSAGE_COMMAND_FLUSH` 复位路径（flush 后
//! 下一帧必为 IDR，丢 1 帧，Resolution 2 旧口径）。线上 key 标志以码流探测
//! `contains_idr` 为唯一权威。
use std::borrow::Cow;
use std::collections::VecDeque;
use std::mem::ManuallyDrop;
use std::sync::Mutex;

use windows::Win32::Foundation::{FALSE, TRUE};
use windows::Win32::Graphics::Direct3D11::{
  ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, ID3D11VideoContext, ID3D11VideoDevice,
  ID3D11VideoProcessor, ID3D11VideoProcessorEnumerator, ID3D11VideoProcessorInputView,
  ID3D11VideoContext1, ID3D11VideoProcessorOutputView, D3D11_BIND_FLAG,
  D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CPU_ACCESS_FLAG,
  D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_TEX2D_VPIV,
  D3D11_TEX2D_VPOV, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
  D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_CONTENT_DESC,
  D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0,
  D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0,
  D3D11_VIDEO_PROCESSOR_STREAM, D3D11_VIDEO_USAGE_PLAYBACK_NORMAL, D3D11_VPIV_DIMENSION_TEXTURE2D,
  D3D11_VPOV_DIMENSION_TEXTURE2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
  DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709, DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709,
  DXGI_FORMAT_NV12, DXGI_RATIONAL,
};
use windows::Win32::Media::MediaFoundation::{
  eAVEncH264VProfile, eAVEncH264VProfile_Base, eAVEncH264VProfile_Main,
  CODECAPI_AVEncCommonMeanBitRate, CODECAPI_AVEncCommonRealTime,
  CODECAPI_AVEncMPVDefaultBPictureCount, CODECAPI_AVEncVideoForceKeyFrame, ICodecAPI,
  IMFDXGIDeviceManager, IMFMediaEventGenerator, MFCreateDXGIDeviceManager,
  MFCreateDXGISurfaceBuffer, MFCreateMemoryBuffer, MFCreateMediaType, MFCreateSample,
  MFSTARTUP_FULL, MF_E_TRANSFORM_NEED_MORE_INPUT, MF_E_TRANSFORM_TYPE_NOT_SET,
  MF_E_NO_EVENTS_AVAILABLE, MF_EVENT_FLAG_NO_WAIT, METransformHaveOutput, METransformNeedInput,
  MFMediaType_Video, MFShutdown, MFStartup,
  MF_VERSION, MFVideoFormat_H264, MFVideoFormat_NV12,
  MFVideoInterlace_Progressive, IMFActivate, IMFMediaType, IMFSample, IMFTransform,
  MFT_CATEGORY_VIDEO_ENCODER,
  MFTEnumEx, MFT_ENUM_FLAG, MFT_ENUM_FLAG_HARDWARE, MFT_ENUM_FLAG_SORTANDFILTER,
  MFT_MESSAGE_COMMAND_DRAIN, MFT_MESSAGE_COMMAND_FLUSH, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING,
  MFT_MESSAGE_NOTIFY_END_OF_STREAM,
  MFT_MESSAGE_NOTIFY_START_OF_STREAM, MFT_MESSAGE_SET_D3D_MANAGER, MFT_OUTPUT_DATA_BUFFER,
  MFT_OUTPUT_DATA_BUFFER_INCOMPLETE, MFT_REGISTER_TYPE_INFO, MF_MT_AVG_BITRATE,
  MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE,
  MF_MT_MPEG2_PROFILE, MF_MT_SUBTYPE, MF_TRANSFORM_ASYNC, MF_TRANSFORM_ASYNC_UNLOCK,
};
use windows::core::{GUID, Interface, IUnknown, VARIANT};

use super::capture::create_texture;
use super::state::{contains_idr, reduce, EncoderAction, EncoderState};
use super::warn;
use crate::error::{AxError, AxResult};

/// GOP 目标（spec「编码」：GOP 120 帧 ≈ 2s@60fps）。MFT 侧无 GOP 属性，由状态机
/// 每 120 帧触发一次 flush-IDR 实现「每 GOP 一个关键帧」。
const GOP_FRAMES: u32 = 120;
/// 90kHz 时间基（MF 视频标准）：sample 时长 = 90000/fps。
const MF_TICKS: i64 = 90_000;

/// MF 进程级引用计数：`MFStartup` 只在首个编码器建立时调用，最后一个编码器
/// drop 时 `MFShutdown`（简报「MFShutdown 对应次数」——一一配对，多次 start/stop
/// 循环后仍平衡；纯 `Once` 在「关了再开」场景会失配，故用计数器承担 Once 角色）。
static MF_USERS: Mutex<usize> = Mutex::new(0);

/// 计数器锁：锁中毒不丢数据（取回守卫继续用——本静态只被短临界区写入，无毒源）。
fn mf_users() -> std::sync::MutexGuard<'static, usize> {
  MF_USERS.lock().unwrap_or_else(|e| e.into_inner())
}

fn mf_acquire() -> AxResult<()> {
  let mut n = mf_users();
  if *n == 0 {
    // 失败语义（简报 Step 4）：MFStartup 失败 = 能力面缺失 → unimplemented。
    unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }
      .map_err(|e| AxError::new("unimplemented", format!("MFStartup 失败: {e}")))?;
  }
  *n += 1;
  Ok(())
}

fn mf_release() {
  let mut n = mf_users();
  *n = n.saturating_sub(1);
  if *n == 0 {
    let _ = unsafe { MFShutdown() };
  }
}

/// 编码输出的一帧（seq/key 由本层与状态机、码流共同决定，wire 组装在 Node 侧）。
pub struct EncodedChunk {
  pub seq: u32,
  pub key: bool,
  /// 采集时刻（Unix 毫秒 u32 截断，worker 取帧时打点）：随帧线上传递，
  /// 主控据此计算端到端帧龄（被控↔主控时钟偏移经 pong 标定）。
  pub captured_at_ms: u32,
  pub data: Vec<u8>,
}

/// VP 输入侧管线（输入尺寸/纹理身份可能在换屏、duplication 重开时变化 → 整体重建；
/// MF 输出侧尺寸不变故不在这里）。绑定 proc 纹理的身份用裸指针值判等——同一 GPU
/// 对象在 duplication 重开后必然是新分配，指针值变化即输入变化。
struct VpPipeline {
  _enumerator: ID3D11VideoProcessorEnumerator,
  vp: ID3D11VideoProcessor,
  input_view: ID3D11VideoProcessorInputView,
  output_view: ID3D11VideoProcessorOutputView,
  nv12: ID3D11Texture2D,
  staging: ID3D11Texture2D,
  bound_texture: usize,
  in_w: u32,
  in_h: u32,
}

/// 强制 IDR 的实现方式：CodecApi（`ICodecAPI` ForceKeyFrame，不丢帧）
/// → 验证未生效即永久降级 Flush（旧路径，丢 1 帧必产 IDR）。
#[derive(Clone, Copy, PartialEq, Eq)]
enum ForceKeyMode {
  CodecApi,
  Flush,
}

pub struct Encoder {
  /// 纹理创建用的原 device（与 video_device 同对象 QI 前的引用）。
  device: ID3D11Device,
  video_device: ID3D11VideoDevice,
  video_ctx: ID3D11VideoContext,
  /// ColorSpace1 变体所在接口（Win10）：设置 RGB→YCbCr limited 的色彩空间。
  video_ctx1: ID3D11VideoContext1,
  ctx: ID3D11DeviceContext,
  out_w: u32,
  out_h: u32,
  fps: u32,
  /// 码率：运行时回退需按系统内存口径重配媒体类型，故随会话保存。
  bitrate_bps: u32,
  mf: IMFTransform,
  /// D3D11↔MF 互操作的设备管理器（仅持有保活；`None` = 系统内存路径）。
  _d3d_manager: Option<IMFDXGIDeviceManager>,
  /// 编码输入是否走 DXGI 表面（运行时回退后翻 false，此后固定系统内存）。
  gpu_interop: bool,
  /// 实际生效的 H.264 profile（能力阶梯的产物；运行期回退重配时保持一致）。
  profile: eAVEncH264VProfile,
  /// ICodecAPI（可用时）：实时提示 + B 帧=0 + 免 flush 强制 IDR；None = 全走回退。
  codec_api: Option<ICodecAPI>,
  /// 强制 IDR 模式（见 `ForceKeyMode`）。
  force_key_mode: ForceKeyMode,
  /// 异步 MFT（MF_TRANSFORM_ASYNC=TRUE；硬件 H.264 编码器普遍如此）必须走事件模型：
  /// 输入只在 METransformNeedInput 后喂、输出只在 METransformHaveOutput 后取——其余
  /// 时机调用 ProcessOutput 返回 E_UNEXPECTED（MS「Asynchronous MFTs」文档）。
  async_mode: bool,
  /// 异步 MFT 的事件源（QI 自 transform；同步 MFT 为 None，drain 直接 no-op）。
  events: Option<IMFMediaEventGenerator>,
  /// 未消费的 METransformNeedInput（异步 MFT 的输入许可）。
  need_input: bool,
  /// 未消费的 METransformHaveOutput 计数（每个事件对应一次 ProcessOutput）。
  have_output: u32,
  /// VP 管线：首帧（见到真实输入纹理）惰性建立。
  vp: Option<VpPipeline>,
  /// 输入→输出配对队列：MF 编码器可能有 0~N 帧流水延迟，输出侧按 FIFO 领回
  /// 自己的 (seq, key, captured_at_ms)；flush 时连同被丢弃的在途输入一起清空。
  pending: VecDeque<(u32, bool, u32)>,
  /// 是否已灌过首帧（首个输入不 Flush——新 transform 自身就是 IDR 起点）。
  pushed: bool,
  /// `VideoProcessorBlt` 的输出帧序号（要求单调递增）。
  frame_counter: u32,
  seq_source: EncoderState,
}

impl Encoder {
  pub fn new(
    device: &ID3D11Device,
    out_w: u32,
    out_h: u32,
    fps: u32,
    bitrate_bps: u32,
  ) -> AxResult<Self> {
    // H.264 约束：偶数尺寸；fps/尺寸非法按入参错误拒绝（调用方 napi 层已校验，
    // 这里是防御——错误尺寸会把错误拖到 MFT SetInputType 才爆，错误归因会失真）。
    let (out_w, out_h) = (out_w & !1, out_h & !1);
    if out_w == 0 || out_h == 0 || fps == 0 || bitrate_bps == 0 {
      return Err(AxError::new("invalid_request", "编码参数非法（尺寸须为正偶数、fps/码率须为正）"));
    }
    mf_acquire()?;
    match Self::build(device, out_w, out_h, fps, bitrate_bps) {
      Ok(s) => Ok(s),
      Err(e) => {
        // build 半途失败时 Drop 不会执行（结构体未构造），手动归还 MF 计数。
        mf_release();
        Err(e)
      }
    }
  }

  fn build(
    device: &ID3D11Device,
    out_w: u32,
    out_h: u32,
    fps: u32,
    bitrate_bps: u32,
  ) -> AxResult<Self> {
    let video_device = device
      .cast::<ID3D11VideoDevice>()
      .map_err(|e| AxError::internal(format!("ID3D11VideoDevice QI 失败: {e}")))?;
    // unsafe 依据（块级）：GetImmediateContext 为 COM FFI。
    let ctx = unsafe { device.GetImmediateContext() }
      .map_err(|e| AxError::internal(format!("GetImmediateContext 失败: {e}")))?;
    let video_ctx = ctx
      .cast::<ID3D11VideoContext>()
      .map_err(|e| AxError::internal(format!("ID3D11VideoContext QI 失败: {e}")))?;
    // ColorSpace1（DXGI 色彩空间口径）只在 ID3D11VideoContext1 上——spec 目标平台
    // Win10（自带 H.264 HW MFT），QI 失败按能力面缺失处理。
    let video_ctx1 = ctx
      .cast::<ID3D11VideoContext1>()
      .map_err(|e| AxError::internal(format!("ID3D11VideoContext1 QI 失败: {e}")))?;
    let (mf, async_mode) = activate_h264_encoder()?;
    // 异步 MFT：QI 事件源（Async MFT 契约要求实现 IMFMediaEventGenerator；QI 失败按
    // 硬性能力缺失收口——没有事件源就无法在事件模型下驱动它）。
    let events = if async_mode {
      Some(
        mf.cast::<IMFMediaEventGenerator>()
          .map_err(|e| AxError::internal(format!("异步编码器缺少事件源接口: {e}")))?,
      )
    } else {
      None
    };
    // 输入路径 × profile 的尝试阶梯（各自独立降级，任一级成功即定型）：
    // ① D3D 互操作+Main ②互操作+Base ③系统内存+Main ④系统内存+Base。
    // 互操作省每帧回读、Main 省码率，两者互不依赖；全部失败 = 旧实现行为与失败面。
    let mut d3d_manager = match setup_d3d_manager(&mf, device) {
      Ok(manager) => Some(manager),
      Err(e) => {
        warn(&format!("D3D 互操作不可用，回退系统内存输入: {e}"));
        None
      }
    };
    let mut gpu_interop = d3d_manager.is_some();
    let mut profile = eAVEncH264VProfile_Main;
    let mut configured = false;
    let ladder: [(bool, eAVEncH264VProfile); 4] = [
      (gpu_interop, eAVEncH264VProfile_Main),
      (gpu_interop, eAVEncH264VProfile_Base),
      (false, eAVEncH264VProfile_Main),
      (false, eAVEncH264VProfile_Base),
    ];
    for (want_gpu, want_profile) in ladder {
      if want_gpu != gpu_interop {
        // 需要系统内存口径：清掉管理器（此后所有重配都以系统内存为准）。
        // unsafe 依据（块级）：COM FFI；清管理器后按系统内存口径重配类型。
        unsafe {
          let _ = mf.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, 0);
        }
        d3d_manager = None;
        gpu_interop = false;
        warn("D3D 互操作类型协商失败，回退系统内存输入");
      }
      if configure_media_types(&mf, out_w, out_h, fps, bitrate_bps, want_profile).is_ok() {
        profile = want_profile;
        configured = true;
        if want_profile == eAVEncH264VProfile_Base {
          warn("H.264 Main profile 不受支持，回退 Baseline");
        }
        break;
      }
    }
    if !configured {
      // 与旧实现同口径的最终失败（以 Base 的错误归因）。
      configure_media_types(&mf, out_w, out_h, fps, bitrate_bps, eAVEncH264VProfile_Base)?;
      profile = eAVEncH264VProfile_Base;
    }
    // 低延迟/免丢帧编码参数（ICodecAPI；逐项尽力而为，不可用则运行期全走回退）。
    let codec_api = apply_codec_api(&mf);
    let force_key_mode = if codec_api.is_some() { ForceKeyMode::CodecApi } else { ForceKeyMode::Flush };
    // unsafe 依据（块级）：IMFTransform 方法均为 COM FFI；transform 持有至 Drop。
    unsafe {
      // 异步 MFT 的流式协议：先 BEGIN_STREAMING 再 START_OF_STREAM（MS「Asynchronous
      // MFTs」）；此后 MFT 以 METransformNeedInput 事件索要输入。
      if async_mode {
        mf.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
          .map_err(|e| AxError::new("unimplemented", format!("编码器进入流式失败: {e}")))?;
      }
      mf.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
        .map_err(|e| AxError::new("unimplemented", format!("编码器开流失败: {e}")))?;
    }
    Ok(Self {
      device: device.clone(),
      video_device,
      video_ctx,
      video_ctx1,
      ctx,
      out_w,
      out_h,
      fps,
      bitrate_bps,
      mf,
      _d3d_manager: d3d_manager,
      gpu_interop,
      profile,
      codec_api,
      force_key_mode,
      async_mode,
      events,
      need_input: false,
      have_output: 0,
      vp: None,
      pending: VecDeque::new(),
      pushed: false,
      frame_counter: 0,
      seq_source: EncoderState::new(GOP_FRAMES),
    })
  }

  /// 编码输出尺寸（换屏时外层据此判断是否需要重建编码器）。
  pub fn out_size(&self) -> (u32, u32) { (self.out_w, self.out_h) }

  /// 换屏重建编码器时取走状态机（seq 必须跨重建连续——wire 的 IV = salt∥seq，
  /// seq 回退 = GCM nonce 复用，是安全契约而非便利）。
  pub fn take_state(&mut self) -> EncoderState {
    std::mem::replace(&mut self.seq_source, EncoderState::new(GOP_FRAMES))
  }

  pub fn import_state(&mut self, state: EncoderState) { self.seq_source = state; }

  pub fn force_idr(&mut self) { self.seq_source.force_idr(); }

  pub fn set_frozen(&mut self, frozen: bool) { self.seq_source.set_frozen(frozen); }

  /// 动态改码率（自适应链路的方向盘；尽力而为）：ICodecAPI 直改 `MeanBitRate`；
  /// 不可用或失败返回 false（保持原码率——自适应是优化不是正确性依赖）。
  /// 记录值同步更新，供运行期回退重配（`disable_gpu_interop`）沿用。
  pub fn set_bitrate(&mut self, bitrate_bps: u32) -> bool {
    if bitrate_bps == 0 {
      return false;
    }
    self.bitrate_bps = bitrate_bps;
    let Some(api) = self.codec_api.as_ref() else {
      return false;
    };
    // unsafe 依据（块级）：ICodecAPI FFI；VARIANT 为本调用临时值。
    unsafe { api.SetValue(&CODECAPI_AVEncCommonMeanBitRate, &VARIANT::from(bitrate_bps)) }.is_ok()
  }

  /// 一帧进、零或一帧出：状态机冻结 → `None`（不喂编码器）；编码器流水未吐
  /// → `None`（继续喂）。`Some` 表示拿到一帧完整 Annex-B ES。
  /// `captured_at_ms` 由调用方在取帧时打点，随配对队列原样随帧输出（帧龄起点）。
  pub fn push(&mut self, texture: &ID3D11Texture2D, captured_at_ms: u32) -> AxResult<Option<EncodedChunk>> {
    // 异步 MFT：先排空事件（上一轮输入的输出事件可能已就绪），再决定是否接受本帧。
    self.drain_events()?;
    if self.async_mode && !self.need_input {
      // 管道忙（无未消费的 NeedInput）：本帧丢弃——实时语义下丢帧优于积压；不消耗
      // seq 状态机、不进配对队列；已就绪的输出照常带走（延迟一个周期的配对不受影响）。
      return self.take_output();
    }
    let (seq, key) = match reduce(&mut self.seq_source) {
      EncoderAction::None => return self.take_output(),
      EncoderAction::Emit { seq, key } => (seq, key),
    };
    // VP 惰性/重建（首帧或输入纹理身份/尺寸变化）。
    self.ensure_vp(texture)?;
    // 即时 IDR：状态机判关键帧（首帧后的 GOP 到期 / force_idr / 换屏 / 解冻 /
    // need-key）。首选 ICodecAPI 的 ForceKeyFrame（下一帧编码为 IDR，不丢帧、不清
    // rate-control 状态）；QI 缺失或设置失败 → 降级 flush 复位路径。首帧不处理
    // （新 transform 本身即 IDR 起点）。
    if key && self.pushed {
      if self.force_key_mode == ForceKeyMode::CodecApi {
        match self.codec_api.as_ref() {
          Some(api) => {
            // unsafe 依据（块级）：ICodecAPI FFI；VARIANT 为本调用临时值。
            if unsafe { api.SetValue(&CODECAPI_AVEncVideoForceKeyFrame, &VARIANT::from(true)) }
              .is_err()
            {
              warn("ForceKeyFrame 设置失败，降级 flush 路径");
              self.force_key_mode = ForceKeyMode::Flush;
            }
          }
          None => self.force_key_mode = ForceKeyMode::Flush,
        }
      }
      if self.force_key_mode == ForceKeyMode::Flush {
        // unsafe 依据（块级）：COM FFI；flush 语义见文件头 Resolution 2 说明。
        unsafe {
          self.mf.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0)
            .map_err(|e| AxError::internal(format!("编码器 Flush 失败: {e}")))?;
          self.mf.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
            .map_err(|e| AxError::internal(format!("编码器重开流失败: {e}")))?;
        }
        // flush 丢弃在途输入，配对队列同步清空（那些 seq 永远不会再有输出）。
        self.pending.clear();
        // flush 同时清空 MFT 事件队列、输入许可作废（异步 MFT；同步无影响）。
        self.need_input = false;
        self.have_output = 0;
        if self.async_mode {
          // 异步 MFT：flush 后须等新的 NeedInput 才可喂输入——本帧丢弃；flush 后
          // 首个被处理的输入必为 IDR，流在下一个被接受的帧上重新对齐关键帧
          // （线上 key 始终以码流 IDR 探测为准，配对的 expected_key 不因丢帧错位）。
          return self.take_output();
        }
      }
    }
    self.pushed = true;
    self.pending.push_back((seq, key, captured_at_ms));
    if self.async_mode {
      // 消费本帧对应的输入许可（喂的就是这一帧）。
      self.need_input = false;
    }
    // GPU：Blt（两路共用）→ 主路产 DXGI 表面样本，回退路回读紧凑 NV12 再灌内存样本。
    let fed = if self.gpu_interop {
      self.blt_to_nv12().and_then(|()| self.feed_input_dxgi(seq))
    } else {
      self.blt_and_readback().and_then(|nv12| self.feed_input(&nv12, seq))
    };
    if let Err(e) = fed {
      if !self.gpu_interop {
        return Err(e);
      }
      // 运行时回退：本驱动不接受 DXGI 输入（建表面样本/ProcessInput 失败）→ 关管理器
      // 走系统内存并重试本帧。回退内的 flush 令重试帧必产 IDR（编码器契约），流在
      // 回退点自然重新对齐关键帧，renderer 无需额外恢复逻辑。
      warn(&format!("D3D 互操作运行中失败，回退系统内存并重试本帧: {e}"));
      self.disable_gpu_interop()?;
      if self.async_mode {
        // 同 flush 路径：回退内的 flush 清空输入许可，本帧丢弃（pending 已被清空），
        // 等新的 NeedInput；此后固定系统内存口径，下一帧起恢复送帧。
        return self.take_output();
      }
      self.pending.push_back((seq, key, captured_at_ms));
      let nv12 = self.blt_and_readback()?;
      self.feed_input(&nv12, seq)?;
    }
    self.take_output()
  }

  /// MF：取一帧已就绪输出并完成配对（seq 来自 `pending` 队首，key 以码流 IDR 探测为
  /// 唯一权威）。异步 MFT 只在有 HaveOutput 事件时真正调用 ProcessOutput（见 pump_output）。
  fn take_output(&mut self) -> AxResult<Option<EncodedChunk>> {
    let Some(data) = self.pump_output()? else { return Ok(None) };
    let data = match to_annex_b(&data) {
      Cow::Borrowed(b) => b.to_vec(),
      Cow::Owned(v) => v,
    };
    // key 的唯一权威 = 码流中的 IDR NAL（flush/GOP/ForceKeyFrame 是否被驱动如实
    // 执行由它兜底）。
    let key = contains_idr(&data);
    match self.pending.pop_front() {
      Some((seq, expected_key, captured_at_ms)) => {
        // 期望关键帧的配对项拿到非 IDR 帧 = ForceKeyFrame 被驱动忽略 → 永久降级
        // flush 路径（丢 1 帧但必产 IDR；恢复确定性优先于少丢一帧）。
        if expected_key && !key && self.force_key_mode == ForceKeyMode::CodecApi {
          warn("ForceKeyFrame 未生效（输出非 IDR），降级 flush 路径");
          self.force_key_mode = ForceKeyMode::Flush;
        }
        Ok(Some(EncodedChunk { seq, key, captured_at_ms, data }))
      }
      // 按构造不可达：每个输出必对应一个入队输入（flush 两侧同步清空）。
      // 若驱动行为破坏该不变式，宁可丢帧也不发错 seq（IV 唯一性优先）。
      None => Ok(None),
    }
  }

  /// 排空 MFT 事件队列（异步 MFT；同步 MFT 为 no-op）：消费 NeedInput / HaveOutput
  /// 事件计数，错误事件上抛（调用方按「重建编码器」收口）。
  fn drain_events(&mut self) -> AxResult<()> {
    let Some(generator) = self.events.as_ref() else {
      return Ok(());
    };
    loop {
      // unsafe 依据（块级）：IMFMediaEventGenerator FFI；非阻塞取事件（NO_WAIT）。
      let event = match unsafe { generator.GetEvent(MF_EVENT_FLAG_NO_WAIT) } {
        Ok(event) => event,
        Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => return Ok(()),
        Err(e) => return Err(AxError::internal(format!("MFT 事件获取失败: {e}"))),
      };
      // unsafe 依据（块级）：IMFMediaEvent FFI；类型/状态为本调用读取。
      let event_type = unsafe { event.GetType() }
        .map_err(|e| AxError::internal(format!("MFT 事件类型读取失败: {e}")))?;
      let status = unsafe { event.GetStatus() }
        .map_err(|e| AxError::internal(format!("MFT 事件状态读取失败: {e}")))?;
      if status.is_err() {
        return Err(AxError::internal(format!("MFT 错误事件（type={event_type}）: {status:?}")));
      }
      if event_type == METransformNeedInput.0 as u32 {
        self.need_input = true;
        continue;
      }
      if event_type == METransformHaveOutput.0 as u32 {
        self.have_output = self.have_output.saturating_add(1);
        continue;
      }
      // METransformDrainComplete 等持久事件不参与驱动：忽略。
    }
  }

  /// 建立/重建 VP 输入侧：输入纹理身份或尺寸变化（换屏、duplication 重开）时
  /// 重建 enumerator/VP/输入视图/NV12 目标；MF 输出侧不动。
  fn ensure_vp(&mut self, texture: &ID3D11Texture2D) -> AxResult<()> {
    // unsafe 依据（块级）：GetDesc 为 COM FFI；desc 为本栈帧槽位。
    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe { texture.GetDesc(&mut desc) };
    let ident = texture.as_raw() as usize;
    if let Some(p) = &self.vp {
      if p.bound_texture == ident && p.in_w == desc.Width && p.in_h == desc.Height {
        return Ok(());
      }
    }
    let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
      InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
      InputFrameRate: DXGI_RATIONAL { Numerator: self.fps, Denominator: 1 },
      InputWidth: desc.Width,
      InputHeight: desc.Height,
      OutputFrameRate: DXGI_RATIONAL { Numerator: self.fps, Denominator: 1 },
      OutputWidth: self.out_w,
      OutputHeight: self.out_h,
      Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
    };
    // unsafe 依据（块级）：ID3D11VideoDevice FFI。
    let enumerator = unsafe { self.video_device.CreateVideoProcessorEnumerator(&content) }
      .map_err(|e| AxError::internal(format!("CreateVideoProcessorEnumerator 失败: {e}")))?;
    let vp = unsafe { self.video_device.CreateVideoProcessor(&enumerator, 0) }
      .map_err(|e| AxError::internal(format!("CreateVideoProcessor 失败: {e}")))?;
    // 色彩管线：输入 = 桌面 RGB 全范围；输出 = NV12 YCbCr 演播室范围（limited，
    // 简报要点「range limited」）、BT.709 色域（与 HD 内容的解码端假设一致）。
    // unsafe 依据（块级）：ID3D11VideoContext1 FFI。
    unsafe {
      self.video_ctx1.VideoProcessorSetStreamColorSpace1(
        &vp,
        0,
        DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709,
      );
      self.video_ctx1
        .VideoProcessorSetOutputColorSpace1(&vp, DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709);
    }
    let iv_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
      FourCC: 0,
      ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
      Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
        Texture2D: D3D11_TEX2D_VPIV { MipSlice: 0, ArraySlice: 0 },
      },
    };
    let mut input_view: Option<ID3D11VideoProcessorInputView> = None;
    unsafe {
      self
        .video_device
        .CreateVideoProcessorInputView(texture, &enumerator, &iv_desc, Some(&mut input_view))
        .map_err(|e| AxError::internal(format!("CreateVideoProcessorInputView 失败: {e}")))?
    };
    let nv12 = create_texture(
      &self.device,
      self.out_w,
      self.out_h,
      DXGI_FORMAT_NV12,
      D3D11_USAGE_DEFAULT,
      D3D11_CPU_ACCESS_FLAG(0),
      // SRV 位：DXGI 表面样本进 D3D 管理器下的编码器时，部分驱动要求纹理可建 SRV。
      D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE,
    )?;
    let staging = create_texture(
      &self.device,
      self.out_w,
      self.out_h,
      DXGI_FORMAT_NV12,
      D3D11_USAGE_STAGING,
      D3D11_CPU_ACCESS_READ,
      D3D11_BIND_FLAG(0),
    )?;
    let ov_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
      ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
      Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
        Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
      },
    };
    let mut output_view: Option<ID3D11VideoProcessorOutputView> = None;
    unsafe {
      self
        .video_device
        .CreateVideoProcessorOutputView(&nv12, &enumerator, &ov_desc, Some(&mut output_view))
        .map_err(|e| AxError::internal(format!("CreateVideoProcessorOutputView 失败: {e}")))?
    };
    self.vp = Some(VpPipeline {
      _enumerator: enumerator,
      vp,
      input_view: input_view.ok_or_else(|| AxError::internal("输入视图为空"))?,
      output_view: output_view.ok_or_else(|| AxError::internal("输出视图为空"))?,
      nv12,
      staging,
      bound_texture: ident,
      in_w: desc.Width,
      in_h: desc.Height,
    });
    Ok(())
  }

  /// `VideoProcessorBlt`：整帧缩放转换到 VP 输出 NV12 纹理（两路共用的唯一 GPU 步骤）。
  fn blt_to_nv12(&mut self) -> AxResult<()> {
    let Some(p) = self.vp.as_ref() else {
      return Err(AxError::internal("VP 管线未建立"));
    };
    let stream = D3D11_VIDEO_PROCESSOR_STREAM {
      Enable: TRUE,
      pInputSurface: ManuallyDrop::new(Some(p.input_view.clone())),
      ..Default::default()
    };
    // unsafe 依据（块级）：VideoProcessorBlt 为 COM FFI；流参数在调用期间存活。
    unsafe {
      self
        .video_ctx
        .VideoProcessorBlt(&p.vp, &p.output_view, self.frame_counter, &[stream])
        .map_err(|e| AxError::internal(format!("VideoProcessorBlt 失败: {e}")))?
    };
    self.frame_counter = self.frame_counter.wrapping_add(1);
    Ok(())
  }

  /// 回退路：staging Map → 紧凑 NV12 回读（系统内存样本用）。
  fn blt_and_readback(&mut self) -> AxResult<Vec<u8>> {
    self.blt_to_nv12()?;
    let Some(p) = self.vp.as_ref() else {
      return Err(AxError::internal("VP 管线未建立"));
    };
    unsafe { self.ctx.CopyResource(&p.staging, &p.nv12) };
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    unsafe {
      self
        .ctx
        .Map(&p.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
        .map_err(|e| AxError::internal(format!("NV12 Map 失败: {e}")))?
    };
    // Map 成功后必须 Unmap——用内部闭包把拷贝收拢，Unmap 无条件执行。
    let read = (|| -> AxResult<Vec<u8>> {
      if mapped.pData.is_null() {
        return Err(AxError::internal("NV12 映射为空"));
      }
      let (w, h) = (self.out_w as usize, self.out_h as usize);
      let pitch = mapped.RowPitch as usize;
      let base = mapped.pData as *const u8; // 空指针转换本身安全，解引用块内已标 unsafe
      let mut out = Vec::with_capacity(w * h * 3 / 2);
      // Y 平面：h 行 × w 字节（行距可能 > w）。
      for r in 0..h {
        let row = unsafe { std::slice::from_raw_parts(base.add(r * pitch), w) };
        out.extend_from_slice(row);
      }
      // UV 平面：紧随 Y 平面（DXGI NV12 = RowPitch × Height 偏移），h/2 行 × w 字节。
      for r in 0..h / 2 {
        let row = unsafe { std::slice::from_raw_parts(base.add(pitch * h + r * pitch), w) };
        out.extend_from_slice(row);
      }
      Ok(out)
    })();
    unsafe { self.ctx.Unmap(&p.staging, 0) };
    read
  }

  /// 紧凑 NV12 → `IMFSample` → `ProcessInput`。
  fn feed_input(&self, nv12: &[u8], seq: u32) -> AxResult<()> {
    let buffer = unsafe { MFCreateMemoryBuffer(nv12.len() as u32) }
      .map_err(|e| AxError::internal(format!("MFCreateMemoryBuffer 失败: {e}")))?;
    let (mut ptr, mut max, mut cur) = (std::ptr::null_mut(), 0u32, 0u32);
    unsafe {
      buffer
        .Lock(&mut ptr, Some(&mut max), Some(&mut cur))
        .map_err(|e| AxError::internal(format!("输入 Buffer Lock 失败: {e}")))?
    };
    if ptr.is_null() {
      // 与相邻路径同纪律：Lock 成功后无论成败都先 Unlock 再返回（防缓冲区卡锁）。
      let _ = unsafe { buffer.Unlock() };
      return Err(AxError::internal("输入 Buffer Lock 返回空指针"));
    }
    // unsafe 依据（块级）：ptr 来自本作用域 Lock、长度 = nv12.len() ≤ max。
    unsafe {
      std::ptr::copy_nonoverlapping(nv12.as_ptr(), ptr, nv12.len());
      buffer
        .Unlock()
        .map_err(|e| AxError::internal(format!("输入 Buffer Unlock 失败: {e}")))?;
    }
    unsafe {
      buffer
        .SetCurrentLength(nv12.len() as u32)
        .map_err(|e| AxError::internal(format!("SetCurrentLength 失败: {e}")))?
    };
    let sample = unsafe { MFCreateSample() }
      .map_err(|e| AxError::internal(format!("MFCreateSample 失败: {e}")))?;
    unsafe {
      sample
        .AddBuffer(&buffer)
        .map_err(|e| AxError::internal(format!("AddBuffer 失败: {e}")))?;
    }
    // 时间基 90kHz；时间戳只要求本次编码会话内单调递增。seq 每次 start 从 0 起
    // （不跨 start 单调）——跨会话的 IV 唯一性由「每次 start 轮换会话盐」保证
    // （IV = 会话盐∥seq，盐轮换是 Task 7/被控 host 的责任），本层只保证会话内不重复。
    self.set_sample_timing(&sample, seq)?;
    unsafe {
      self
        .mf
        .ProcessInput(0, &sample, 0)
        .map_err(|e| AxError::internal(format!("ProcessInput 失败: {e}")))?;
    }
    Ok(())
  }

  /// 样本时间戳（两路共用）：90kHz 时间基，seq 单调 ⇒ 时间轴单调。
  fn set_sample_timing(&self, sample: &IMFSample, seq: u32) -> AxResult<()> {
    let t = (seq as i64) * MF_TICKS / self.fps as i64;
    unsafe {
      sample
        .SetSampleTime(t)
        .map_err(|e| AxError::internal(format!("SetSampleTime 失败: {e}")))?;
      sample
        .SetSampleDuration(MF_TICKS / self.fps as i64)
        .map_err(|e| AxError::internal(format!("SetSampleDuration 失败: {e}")))?;
    }
    Ok(())
  }

  /// 主路输入：NV12 纹理直接包成 DXGI 表面样本（零回读零拷贝）。
  fn feed_input_dxgi(&self, seq: u32) -> AxResult<()> {
    let Some(p) = self.vp.as_ref() else {
      return Err(AxError::internal("VP 管线未建立"));
    };
    let surface: IUnknown = p
      .nv12
      .cast()
      .map_err(|e| AxError::new("unimplemented", format!("NV12 纹理转 IUnknown 失败: {e}")))?;
    // unsafe 依据（块级）：MFCreateDXGISurfaceBuffer 为 MF FFI；纹理由 VP 管线持有，
    // D3D 管理器下编码器按契约消费该表面（表面引用由样本持有至编码完成）。
    let buffer = unsafe { MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, &surface, 0, FALSE) }
      .map_err(|e| AxError::new("unimplemented", format!("MFCreateDXGISurfaceBuffer 失败: {e}")))?;
    let sample = unsafe { MFCreateSample() }
      .map_err(|e| AxError::internal(format!("MFCreateSample 失败: {e}")))?;
    unsafe {
      sample
        .AddBuffer(&buffer)
        .map_err(|e| AxError::internal(format!("AddBuffer 失败: {e}")))?;
    }
    self.set_sample_timing(&sample, seq)?;
    unsafe {
      self
        .mf
        .ProcessInput(0, &sample, 0)
        .map_err(|e| AxError::internal(format!("ProcessInput 失败: {e}")))?;
    }
    Ok(())
  }

  /// 运行时回退：flush → 清 D3D 管理器 → 系统内存口径重配类型 → 重开流。
  /// flush 丢弃在途输入（其输出不再回来），pending 清空；调用方随后重试本帧
  /// （flush 后首帧必为 IDR，线上 key 仍以码流探测为准）。
  fn disable_gpu_interop(&mut self) -> AxResult<()> {
    // unsafe 依据（块级）：MF COM FFI；transform 持有至 Drop。
    unsafe {
      self
        .mf
        .ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0)
        .map_err(|e| AxError::internal(format!("回退 flush 失败: {e}")))?;
      self
        .mf
        .ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, 0)
        .map_err(|e| AxError::internal(format!("回退清 D3D 管理器失败: {e}")))?;
    }
    configure_media_types(&self.mf, self.out_w, self.out_h, self.fps, self.bitrate_bps, self.profile)?;
    // 类型重协商可能重置 codec 级设置：重放（QI 不可用则强制降级 flush 模式）。
    self.codec_api = apply_codec_api(&self.mf);
    if self.codec_api.is_none() {
      self.force_key_mode = ForceKeyMode::Flush;
    }
    // unsafe 依据（块级）：同上，COM FFI。
    unsafe {
      self
        .mf
        .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
        .map_err(|e| AxError::internal(format!("回退重开流失败: {e}")))?;
    }
    // flush 清空 MFT 事件队列与输入许可（异步 MFT：等新的 NeedInput 再喂）。
    self.need_input = false;
    self.have_output = 0;
    self._d3d_manager = None;
    self.gpu_interop = false;
    self.pending.clear();
    Ok(())
  }

  /// `ProcessOutput` 循环：拿到一帧含数据的 ES 或「本轮无输出」（NEED_MORE_INPUT）。
  /// 异步 MFT：仅在存在未消费 METransformHaveOutput 事件时才调用，且每个事件对应一次
  /// 调用——其余时机 ProcessOutput 返回 E_UNEXPECTED（MS「Asynchronous MFTs」）。
  fn pump_output(&mut self) -> AxResult<Option<Vec<u8>>> {
    let cap = (self.out_w as usize) * (self.out_h as usize) * 3 / 2;
    let iterations = if self.async_mode {
      if self.have_output == 0 {
        return Ok(None);
      }
      1
    } else {
      8
    };
    for _ in 0..iterations {
      if self.async_mode {
        self.have_output -= 1;
      }
      // 输出样本自备（除非驱动声明 PROVIDES_SAMPLES——那种情形下传 None 也安全，
      // 统一自备并容忍 MFT 覆盖 pSample）。
      let out_sample = self.make_output_sample(cap)?;
      let mut out = MFT_OUTPUT_DATA_BUFFER {
        dwStreamID: 0,
        pSample: ManuallyDrop::new(Some(out_sample)),
        dwStatus: 0,
        pEvents: ManuallyDrop::new(None),
      };
      let mut status = 0u32;
      let r = unsafe { self.mf.ProcessOutput(0, std::slice::from_mut(&mut out), &mut status) };
      let flags = out.dwStatus;
      let produced = ManuallyDrop::into_inner(out.pSample);
      let events = ManuallyDrop::into_inner(out.pEvents);
      drop(events); // 编码器不发事件；即使发了也立即释放引用
      match r {
        Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(None),
        Err(e) => return Err(AxError::internal(format!("ProcessOutput 失败: {e}"))),
        Ok(()) => {}
      }
      if let Some(smp) = produced {
        let data = read_sample_bytes(&smp)?;
        if !data.is_empty() {
          return Ok(Some(data));
        }
      }
      if flags & (MFT_OUTPUT_DATA_BUFFER_INCOMPLETE.0 as u32) != 0 {
        continue; // 还有输出待取（同一次输入的后续数据）
      }
      return Ok(None);
    }
    Ok(None)
  }

  fn make_output_sample(&self, cap: usize) -> AxResult<IMFSample> {
    let buffer = unsafe { MFCreateMemoryBuffer(cap as u32) }
      .map_err(|e| AxError::internal(format!("输出 Buffer 创建失败: {e}")))?;
    let sample = unsafe { MFCreateSample() }
      .map_err(|e| AxError::internal(format!("输出 Sample 创建失败: {e}")))?;
    unsafe {
      sample
        .AddBuffer(&buffer)
        .map_err(|e| AxError::internal(format!("输出 AddBuffer 失败: {e}")))?
    };
    Ok(sample)
  }
}

// SAFETY：`Encoder` 只在「创建线程 → 单一采集线程」间 move 一次，之后所有方法
// 都只在该采集线程上调用（`napi.rs` worker 独占持有，无并发访问）。MF 的
// `IMFTransform` 按 Media Foundation 契约是 agile 对象（MFT 允许任意线程调用、
// 客户端自行串行化），不存在线程亲和性；windows 0.58 未给 MF 接口标 `Send` 属
// 元数据缺口而非语义禁止（同 crate 的 DXGI/D3D11 接口均显式标了 Send）。
unsafe impl Send for Encoder {}

impl Drop for Encoder {
  fn drop(&mut self) {
    // 收口 drain（简报要点 6）：通知流结束并排空缓冲；尽力而为，失败不改结果
    // （transform 释放时由 MF 内部清理）。
    unsafe {
      let _ = self.mf.ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
      let _ = self.mf.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0);
    }
    mf_release();
  }
}

/// 建立 D3D11↔MF 互操作：创建 DXGI 设备管理器并把 D3D11 device 挂进去，再经
/// `MFT_MESSAGE_SET_D3D_MANAGER` 交给编码器（此后编码器输入样本必须是同一 device
/// 上的 DXGI 表面缓冲）。任一步失败返回 Err，由调用方回退系统内存路径。
fn setup_d3d_manager(mf: &IMFTransform, device: &ID3D11Device) -> AxResult<IMFDXGIDeviceManager> {
  let mut reset_token = 0u32;
  let mut manager: Option<IMFDXGIDeviceManager> = None;
  // unsafe 依据（块级）：MF COM FFI；出参槽位本栈帧持有，成功引用由本函数接管。
  unsafe { MFCreateDXGIDeviceManager(&mut reset_token, &mut manager) }
    .map_err(|e| AxError::internal(format!("MFCreateDXGIDeviceManager 失败: {e}")))?;
  let manager = manager.ok_or_else(|| AxError::internal("DXGI 设备管理器为空"))?;
  let unknown: IUnknown = device
    .cast()
    .map_err(|e| AxError::internal(format!("ID3D11Device 转 IUnknown 失败: {e}")))?;
  // unsafe 依据（块级）：COM FFI；unknown 与 reset_token 由本函数持有/生成。
  unsafe { manager.ResetDevice(&unknown, reset_token) }
    .map_err(|e| AxError::internal(format!("设备管理器 ResetDevice 失败: {e}")))?;
  // 进程内 MFT 按契约接收接口指针本身（非封送流指针）。
  // unsafe 依据（块级）：IMFTransform FFI；manager 在函数返回前保持存活。
  unsafe { mf.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, manager.as_raw() as usize) }
    .map_err(|e| AxError::internal(format!("编码器拒绝 D3D 管理器: {e}")))?;
  Ok(manager)
}

/// 检测并解锁异步 MFT（返回是否异步）：硬件 H.264 MFT（NVENC / AMF / QSV）普遍是
/// 异步编码器——读 `MF_TRANSFORM_ASYNC` 判定；异步时在配置媒体类型前置
/// `MF_TRANSFORM_ASYNC_UNLOCK=TRUE`，否则首次 `SetInputType` 报
/// `MF_E_TRANSFORM_ASYNC_LOCKED (0xC00D6D77)`（现象：会话连上但画面永远不出）。
/// 同步 MFT 无该属性（读取失败即同步）；解锁后还必须按事件模型驱动（见 Encoder 字段
/// `async_mode` 注释），解锁只是入场券。MS 参考：MF_TRANSFORM_ASYNC_UNLOCK / Asynchronous MFTs。
fn unlock_async_mft(mf: &IMFTransform) -> bool {
  // unsafe 依据（块级）：IMFTransform/IMFAttributes 均为 COM FFI；属性接口仅本函数使用。
  unsafe {
    let Ok(attributes) = mf.GetAttributes() else {
      return false;
    };
    // 缺属性（同步 MFT）→ 读取失败 → 不需要解锁。
    let is_async = matches!(attributes.GetUINT32(&MF_TRANSFORM_ASYNC), Ok(value) if value != 0);
    if is_async {
      // 解锁失败也无后续补救路径；保持原错误面（首个 Set 调用会报 ASYNC_LOCKED）。
      let _ = attributes.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1);
    }
    is_async
  }
}

/// 枚举硬件 H.264 视频编码器并激活。返回 (transform, 是否异步 MFT)。
///
/// 选择口径：简报写「取含 "H.264" 描述者」，但 windows 0.58 元数据没有
/// `MF_TRANSFORM_FRIENDLY_NAME` 常量——改用等价且更强的类型过滤
/// （输入 NV12 + 输出 H264 + `SORTANDFILTER` 排序），首个可激活且可配类型的胜出。
fn activate_h264_encoder() -> AxResult<(IMFTransform, bool)> {
  let input_info = MFT_REGISTER_TYPE_INFO {
    guidMajorType: MFMediaType_Video,
    guidSubtype: MFVideoFormat_NV12,
  };
  let output_info = MFT_REGISTER_TYPE_INFO {
    guidMajorType: MFMediaType_Video,
    guidSubtype: MFVideoFormat_H264,
  };
  let flags =
    MFT_ENUM_FLAG(MFT_ENUM_FLAG_HARDWARE.0 | MFT_ENUM_FLAG_SORTANDFILTER.0);
  let mut arr: *mut Option<IMFActivate> = std::ptr::null_mut();
  let mut count = 0u32;
  // unsafe 依据（块级）：MFTEnumEx 分配激活器数组，本函数负责逐个取用后
  // CoTaskMemFree 数组；激活器引用经 ptr::read 转移（不额外 AddRef）。
  unsafe {
    MFTEnumEx(
      MFT_CATEGORY_VIDEO_ENCODER,
      flags,
      Some(&input_info),
      Some(&output_info),
      &mut arr,
      &mut count,
    )
    .map_err(|e| AxError::new("unimplemented", format!("MFTEnumEx(硬件 H.264) 失败: {e}")))?;
  }
  let mut activated: Option<IMFTransform> = None;
  let mut activated_async = false;
  if !arr.is_null() {
    for i in 0..count as usize {
      // 数组元素 = 单个接口指针槽（8B，与 Option<IMFActivate> 同布局），按 Option
      // 读出转移所有权：选中的留下，其余随作用域 drop 释放引用（不额外 AddRef）。
      let act = unsafe { std::ptr::read(arr.add(i)) };
      let Some(activate) = act else { continue };
      if activated.is_some() {
        continue;
      }
      match unsafe { activate.ActivateObject::<IMFTransform>() } {
        Ok(t) => {
          // 激活成功即检测/解锁异步 MFT（配置媒体类型之前必须完成，见 unlock_async_mft 注释）。
          activated_async = unlock_async_mft(&t);
          activated = Some(t)
        }
        // 单个激活失败（驱动状态异常）→ 尝试下一个候选。
        Err(_) => continue,
      }
    }
    unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(arr as *const core::ffi::c_void)) };
  }
  activated
    .map(|transform| (transform, activated_async))
    .ok_or_else(|| {
      AxError::new("unimplemented", "无可用硬件 H.264 编码器（MFTEnumEx 无候选或激活失败）")
    })
}

/// 配置输入/输出 media type（MS H.264 编码器口径：输入 NV12+尺寸+帧率+逐行；
/// 输出 H264+profile（调用方按能力阶梯选择，Main 优先）+码率）。
fn configure_media_types(
  mf: &IMFTransform,
  out_w: u32,
  out_h: u32,
  fps: u32,
  bitrate_bps: u32,
  profile: eAVEncH264VProfile,
) -> AxResult<()> {
  let input = unsafe { MFCreateMediaType() }
    .map_err(|e| AxError::new("unimplemented", format!("MFCreateMediaType(输入) 失败: {e}")))?;
  // unsafe 依据（块级）：IMFAttributes 属性写入为 COM FFI。
  unsafe {
    input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
      .map_err(|e| AxError::new("unimplemented", format!("输入类型 major 失败: {e}")))?
  };
  // unsafe 依据（块级）：IMFAttributes 属性写入为 COM FFI。
  unsafe {
    input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)
      .map_err(|e| AxError::new("unimplemented", format!("输入类型 subtype 失败: {e}")))?
  };
  // unsafe 依据（块级）：IMFAttributes 属性写入为 COM FFI。
  unsafe {
    input.SetUINT64(&MF_MT_FRAME_SIZE, ((out_w as u64) << 32) | out_h as u64)
      .map_err(|e| AxError::new("unimplemented", format!("输入类型 frame size 失败: {e}")))?
  };
  // unsafe 依据（块级）：IMFAttributes 属性写入为 COM FFI。
  unsafe {
    input.SetUINT64(&MF_MT_FRAME_RATE, ((fps as u64) << 32) | 1)
      .map_err(|e| AxError::new("unimplemented", format!("输入类型 frame rate 失败: {e}")))?
  };
  // unsafe 依据（块级）：IMFAttributes 属性写入为 COM FFI。
  unsafe {
    input.SetUINT32(
      &MF_MT_INTERLACE_MODE,
      MFVideoInterlace_Progressive.0 as u32,
    )
      .map_err(|e| AxError::new("unimplemented", format!("输入类型 interlace 失败: {e}")))?
  };

  let output = unsafe { MFCreateMediaType() }
    .map_err(|e| AxError::new("unimplemented", format!("MFCreateMediaType(输出) 失败: {e}")))?;
  // unsafe 依据（块级）：IMFAttributes 属性写入为 COM FFI。
  unsafe {
    output.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
      .map_err(|e| AxError::new("unimplemented", format!("输出类型 major 失败: {e}")))?
  };
  // unsafe 依据（块级）：IMFAttributes 属性写入为 COM FFI。
  unsafe {
    output.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)
      .map_err(|e| AxError::new("unimplemented", format!("输出类型 subtype 失败: {e}")))?
  };
  // unsafe 依据（块级）：IMFAttributes 属性写入为 COM FFI。
  unsafe {
    output.SetUINT64(&MF_MT_FRAME_SIZE, ((out_w as u64) << 32) | out_h as u64)
      .map_err(|e| AxError::new("unimplemented", format!("输出类型 frame size 失败: {e}")))?
  };
  // unsafe 依据（块级）：IMFAttributes 属性写入为 COM FFI。
  unsafe {
    output.SetUINT64(&MF_MT_FRAME_RATE, ((fps as u64) << 32) | 1)
      .map_err(|e| AxError::new("unimplemented", format!("输出类型 frame rate 失败: {e}")))?
  };
  // unsafe 依据（块级）：IMFAttributes 属性写入为 COM FFI。
  unsafe {
    output.SetUINT32(&MF_MT_AVG_BITRATE, bitrate_bps)
      .map_err(|e| AxError::new("unimplemented", format!("输出类型码率失败: {e}")))?
  };
  // unsafe 依据（块级）：IMFAttributes 属性写入为 COM FFI。
  unsafe {
    output.SetUINT32(
      &MF_MT_INTERLACE_MODE,
      MFVideoInterlace_Progressive.0 as u32,
    )
      .map_err(|e| AxError::new("unimplemented", format!("输出类型 interlace 失败: {e}")))?
  };
  // profile 由调用方按能力阶梯选择（Main 优先，驱动拒绝降级 Base；Main 带 CABAC
  // 同码率画质更高）。无 B 帧由 `apply_codec_api` 显式置 0——Main 下无 B 帧才能
  // 保持输入/输出 1:1 配对与最低编码延迟（MF_MT_MPEG2_LEVEL 按简报省略）。
  // unsafe 依据（块级）：IMFAttributes 属性写入为 COM FFI。
  unsafe {
    output.SetUINT32(&MF_MT_MPEG2_PROFILE, profile.0 as u32)
      .map_err(|e| AxError::new("unimplemented", format!("输出类型 profile 失败: {e}")))?
  };
  set_codec_media_types(&mf, &input, &output)
}

/// 设置输入/输出媒体类型；顺序随编码 MFT 而异（MF 文档：顺序不对报
/// `MF_E_TRANSFORM_TYPE_NOT_SET (0xC00D6D60)`——部分硬件编码 MFT 的输入流依赖输出流
/// 先成形（帧尺寸/profile 由输出类型定义），必须先 SetOutputType）。
/// 先按「输入→输出」；命中 TYPE_NOT_SET 即换「输出→输入」重试——首个 Set 失败不消费
/// 类型对象，换序重设即告完成（2026-10-13：AMD 硬编 MFT 需输出先行，漏了这一步的
/// 现象是 `rd_media_start` 失败、会话连上但画面永远不出）。
fn set_codec_media_types(
  mf: &IMFTransform,
  input: &IMFMediaType,
  output: &IMFMediaType,
) -> AxResult<()> {
  let set_input = || unsafe { mf.SetInputType(0, input, 0) };
  let set_output = || unsafe { mf.SetOutputType(0, output, 0) };
  match set_input() {
    Ok(()) => set_output()
      .map_err(|e| AxError::new("unimplemented", format!("SetOutputType 失败: {e}"))),
    Err(error) if error.code() == MF_E_TRANSFORM_TYPE_NOT_SET => {
      set_output()
        .map_err(|e| AxError::new("unimplemented", format!("SetOutputType 失败: {e}")))?;
      set_input().map_err(|e| AxError::new("unimplemented", format!("SetInputType 失败: {e}")))
    }
    Err(error) => Err(AxError::new(
      "unimplemented",
      format!("SetInputType 失败: {error}"),
    )),
  }
}

/// 低延迟 / 免丢帧的编码参数（`ICodecAPI`；逐项尽力而为，单项失败只提示不失败）：
/// - `AVEncCommonRealTime=TRUE`：实时模式提示（时延优先，允许驱动按时延取舍）；
/// - `AVEncMPVDefaultBPictureCount=0`：显式禁 B 帧（Main/High 下保持输入输出 1:1
///   配对与最低延迟；Baseline 本无 B 帧，接受或忽略均无害）。
///
/// 返回接口实例供运行期 `CODECAPI_AVEncVideoForceKeyFrame`；QI 失败 = `None`
/// （运行期全回退：强制 IDR 走 flush 路径）。
fn apply_codec_api(mf: &IMFTransform) -> Option<ICodecAPI> {
  let api: ICodecAPI = mf.cast().ok()?;
  let set = |guid: &GUID, value: VARIANT| -> bool {
    // unsafe 依据（块级）：ICodecAPI FFI；guid/value 为本调用临时值（仅读取）。
    unsafe { api.SetValue(guid, &value) }.is_ok()
  };
  if !set(&CODECAPI_AVEncCommonRealTime, VARIANT::from(true)) {
    warn("编码器拒绝实时模式提示（继续）");
  }
  if !set(&CODECAPI_AVEncMPVDefaultBPictureCount, VARIANT::from(0u32)) {
    warn("编码器拒绝 B 帧数设置（继续）");
  }
  Some(api)
}

/// 从 `IMFSample` 读出连续 ES 字节。
fn read_sample_bytes(sample: &IMFSample) -> AxResult<Vec<u8>> {
  let buffer = unsafe { sample.ConvertToContiguousBuffer() }
    .map_err(|e| AxError::internal(format!("ConvertToContiguousBuffer 失败: {e}")))?;
  let (mut ptr, mut max, mut cur) = (std::ptr::null_mut(), 0u32, 0u32);
  unsafe {
    buffer
      .Lock(&mut ptr, Some(&mut max), Some(&mut cur))
      .map_err(|e| AxError::internal(format!("输出 Buffer Lock 失败: {e}")))?
  };
  if ptr.is_null() {
    // 与相邻路径同纪律：Lock 成功后无论成败都先 Unlock 再返回（防缓冲区卡锁）。
    let _ = unsafe { buffer.Unlock() };
    return Err(AxError::internal("输出 Buffer Lock 返回空指针"));
  }
  // unsafe 依据（块级）：ptr/cur 来自 Lock 返回值，锁保持到本作用域 Unlock。
  let bytes = unsafe { std::slice::from_raw_parts(ptr, cur as usize) }.to_vec();
  unsafe {
    buffer
      .Unlock()
      .map_err(|e| AxError::internal(format!("输出 Buffer Unlock 失败: {e}")))?
  };
  Ok(bytes)
}

/// 把编码器输出规整成 Annex-B（起始码 `00 00 00 01` 分隔）。
///
/// WHY：renderer 侧 WebCodecs 按「无 description = Annex-B」配置（spec「主控
/// renderer」），而 MF H.264 编码器的输出 NAL 封装形态随实现/驱动存在 Annex-B 与
/// AVCC（4 字节大端长度前缀）两种——本函数是两者之间唯一的安全网：
/// - 已是 Annex-B → 原样借用（零拷贝）；
/// - 严格匹配 AVCC（长度序列完整消费全缓冲、NAL 类型合法）→ 转起始码；
/// - 其它任何形态 → 原样返回（宁可交给上层失败，也不猜着重排破坏字节）。
///
/// 纯函数，tests/rd_media.rs 覆盖两种路径。
pub fn to_annex_b(data: &[u8]) -> Cow<'_, [u8]> {
  if data.len() >= 3
    && data[0] == 0
    && data[1] == 0
    && (data[2] == 1 || (data[2] == 0 && data.len() >= 4 && data[3] == 1))
  {
    return Cow::Borrowed(data);
  }
  let mut out: Vec<u8> = Vec::with_capacity(data.len() + 16);
  let mut pos = 0usize;
  while pos + 4 <= data.len() {
    let n = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
    if n == 0 || pos + 4 + n > data.len() {
      return Cow::Borrowed(data); // 长度越界 → 不是 AVCC
    }
    let nal = &data[pos + 4..pos + 4 + n];
    let nal_type = nal[0] & 0x1f;
    if nal_type == 0 || nal_type > 30 {
      return Cow::Borrowed(data); // NAL 类型非法 → 不是 AVCC
    }
    out.extend_from_slice(&[0, 0, 0, 1]);
    out.extend_from_slice(nal);
    pos += 4 + n;
  }
  if pos == data.len() && pos > 0 {
    Cow::Owned(out)
  } else {
    Cow::Borrowed(data)
  }
}
