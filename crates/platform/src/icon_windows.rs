//! `getApplicationIcon` 的 **Windows 原生解码**（M3 Task 4 · DR-T4-2）。
//!
//! 规格：`docs/specs/tauri-platform-commands.md` 的 `getApplicationIcon` 一节。
//! 契约是 `Promise<ApplicationIconInfo | null>`——**取不到就是 `null`，绝不抛**
//! （调用方已经处理了可空，见 `packages/shared/src/platform.ts` 的声明与 M1 的取值口径）。
//!
//! ## 取图标的路径
//!
//! ```text
//! locator（按顺序试）
//!   └─ 解析出**可执行文件路径**
//!        ├─ windows-executable-path：路径直接存在就用它
//!        ├─ windows-aumid：先当路径试，再查 `App Paths`（见下）
//!        └─ darwin-bundle-id：Windows 上无意义 → 跳过（macOS 半边本阶段不做，见 spec）
//!   └─ SHGetFileInfoW(SHGFI_ICON|SHGFI_LARGEICON) 取 HICON
//!   └─ GetIconInfo + GetDIBits 取 32bpp 像素（BGRA，top-down）
//!   └─ BGRA → RGBA → PNG → base64 → `data:image/png;base64,…`
//! ```
//!
//! ## 已知边界（登记，不假装）
//!
//! - **AUMID → 可执行文件没有公开解析 API**：Windows 没有「给定 AppUserModelID 求它的 exe」
//!   的官方接口（AUMID 由应用自己在运行时声明/清单里写，反查需要 COM 激活那一套）。
//!   本实现把 AUMID 当**文件名/路径**处理，并查 `App Paths`（`HKLM`/`HKCU` 的
//!   `SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\<名字>.exe`）——这条覆盖
//!   「按 exe 名/应用名取图标」的常见用法，取不到就返回 `null`（契约允许）。
//! - **macOS 半边未实现**（Ruling 20）：`darwin-bundle-id` 在 Windows 上只能是 `null`。
//! - **单色图标**（只有 AND 掩码、没有颜色位图）返回 `null`：它们没有可用的颜色数据，
//!   硬凑一张黑白图会把「取不到」伪装成「取到了」。
#![cfg(target_os = "windows")]

use std::path::Path;

use base64::Engine as _;
use protocol::ApplicationIconLocator;

use windows_sys::Win32::Foundation::WIN32_ERROR;
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP,
};
use windows_sys::Win32::System::Registry::{
    RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ,
};
use windows_sys::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON};
use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, ICONINFO};

/// `App Paths` 的键路径（Windows 上「按可执行文件名字查完整路径」的既有机制）。
const APP_PATHS_SUBKEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths";

/// data URL 的前缀。**媒体类型是契约的一部分**：调用方把它塞进 `<img src>`，
/// 所以这里给的是浏览器认得的 PNG。
pub const PNG_DATA_URL_PREFIX: &str = "data:image/png;base64,";

/// 按 locator 顺序取图标，第一个取到的即返回。
pub fn icon_data_url(locators: &[ApplicationIconLocator]) -> Option<String> {
    for locator in locators {
        let Some(executable) = resolve_executable(locator) else {
            continue;
        };
        if let Some(data_url) = png_data_url_for_file(&executable) {
            return Some(data_url);
        }
    }
    None
}

/// locator → 可执行文件路径（不存在/不适用时 `None`，由调用方跳到下一个 locator）。
pub fn resolve_executable(locator: &ApplicationIconLocator) -> Option<String> {
    match locator {
        ApplicationIconLocator::WindowsExecutablePath { value } => resolve_path_or_app_paths(value),
        // 见模块文档：AUMID 没有公开的反查 API，按「文件名/路径」处理。
        ApplicationIconLocator::WindowsAumid { value } => resolve_path_or_app_paths(value),
        // macOS 半边未实现（Ruling 20）。
        ApplicationIconLocator::DarwinBundleId { .. } => None,
    }
}

/// 路径直接存在就用它；否则按 `App Paths` 查（先 HKCU 后 HKLM，与系统查找顺序一致）。
pub fn resolve_path_or_app_paths(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if Path::new(trimmed).is_file() {
        return Some(trimmed.to_string());
    }
    // 只有「看起来是程序名」的输入才查 App Paths：带分隔符的路径查不到也只是白跑一次。
    let name = Path::new(trimmed)
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())?;
    let key = format!("{APP_PATHS_SUBKEY}\\{name}");
    for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        if let Some(path) = registry_string(root, &key) {
            if Path::new(&path).is_file() {
                return Some(path);
            }
        }
    }
    None
}

/// 读一个 REG_SZ 值（默认值：`lpvalue` 传 `None`）。失败/空串都返回 `None`，不抛。
fn registry_string(root: HKEY, subkey: &str) -> Option<String> {
    let subkey_wide = to_wide(subkey);
    let mut buffer = [0u16; 512];
    let mut size = std::mem::size_of_val(&buffer) as u32;
    let status: WIN32_ERROR = unsafe {
        RegGetValueW(
            root,
            subkey_wide.as_ptr(),
            std::ptr::null(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if status != 0 {
        return None;
    }
    // `size` 是**字节数**（含结尾的 NUL），且可能超出我们给的缓冲（截断）——截断的路径不可用。
    let units = (size as usize) / 2;
    if units == 0 || units > buffer.len() {
        return None;
    }
    let end = buffer[..units].iter().position(|unit| *unit == 0).unwrap_or(units);
    let value = String::from_utf16_lossy(&buffer[..end]);
    let trimmed = value.trim().to_string();
    (!trimmed.is_empty()).then_some(trimmed)
}

/// 取一个文件的大图标并编码成 PNG data URL。任一步失败返回 `None`（不抛）。
pub fn png_data_url_for_file(path: &str) -> Option<String> {
    let icon = shell_large_icon(path)?;
    // `IconGuard` 负责 DestroyIcon：下面任何一条 `?` 提前返回都不会漏掉释放。
    let guard = IconGuard(icon);
    let rgba = rgba_from_icon(guard.0)?;
    data_url_from_rgba(rgba.width, rgba.height, &rgba.pixels)
}

/// `SHGetFileInfoW` 取大图标（HICON）。
fn shell_large_icon(path: &str) -> Option<*mut core::ffi::c_void> {
    let path_wide = to_wide(path);
    let mut info = SHFILEINFOW::default();
    let result = unsafe {
        SHGetFileInfoW(
            path_wide.as_ptr(),
            0,
            &mut info,
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        )
    };
    (result != 0 && !info.hIcon.is_null()).then_some(info.hIcon)
}

/// `DestroyIcon` 的 RAII 包装（HICON 必须释放，否则每次调用漏一份图标资源）。
struct IconGuard(*mut core::ffi::c_void);

impl Drop for IconGuard {
    fn drop(&mut self) {
        unsafe {
            DestroyIcon(self.0);
        }
    }
}

/// 一张 RGBA8 位图。
struct RgbaImage {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

/// HICON → RGBA8 像素（含「颜色位图没有 alpha 时用 AND 掩码补」的那一档）。
fn rgba_from_icon(icon: *mut core::ffi::c_void) -> Option<RgbaImage> {
    let mut info = ICONINFO::default();
    if unsafe { GetIconInfo(icon, &mut info) } == 0 {
        return None;
    }
    // 两个位图都由我们负责释放（GetIconInfo 的约定）。
    let color_guard = BitmapGuard(info.hbmColor);
    let mask_guard = BitmapGuard(info.hbmMask);
    if color_guard.0.is_null() {
        // 单色图标：没有颜色位图（见模块文档，返回 null 而不是硬凑黑白图）。
        return None;
    }

    let (width, height) = bitmap_size(color_guard.0)?;
    let mut pixels = dib_pixels(color_guard.0, width, height)?;
    if pixels.iter().skip(3).step_by(4).all(|alpha| *alpha == 0) {
        // 颜色位图整块 alpha 为 0（老式图标）：按 GDI 的语义用 AND 掩码当透明度——
        // 掩码位为 0 表示「用颜色位图」，即不透明。
        apply_mask_alpha(&mut pixels, &mask_guard.0, width, height);
    }
    bgra_to_rgba(&mut pixels);
    Some(RgbaImage {
        width,
        height,
        pixels,
    })
}

/// `DeleteObject` 的 RAII 包装（HBITMAP 必须释放）。
struct BitmapGuard(HBITMAP);

impl Drop for BitmapGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                DeleteObject(self.0);
            }
        }
    }
}

/// 内存设备上下文（`GetDIBits` 需要一个真 DC，见那里的实测说明）。
struct DeviceContextGuard(windows_sys::Win32::Graphics::Gdi::HDC);

impl DeviceContextGuard {
    fn create() -> Option<Self> {
        let dc = unsafe { CreateCompatibleDC(std::ptr::null_mut()) };
        (!dc.is_null()).then_some(Self(dc))
    }
}

impl Drop for DeviceContextGuard {
    fn drop(&mut self) {
        unsafe {
            DeleteDC(self.0);
        }
    }
}

/// 位图尺寸（`GetObjectW` 读 `BITMAP`）。
fn bitmap_size(bitmap: HBITMAP) -> Option<(u32, u32)> {
    let mut details = BITMAP::default();
    let read = unsafe {
        GetObjectW(
            bitmap,
            std::mem::size_of::<BITMAP>() as i32,
            (&mut details as *mut BITMAP).cast(),
        )
    };
    if read == 0 || details.bmWidth <= 0 || details.bmHeight <= 0 {
        return None;
    }
    Some((details.bmWidth as u32, details.bmHeight as u32))
}

/// `GetDIBits` 取 32bpp 的 BGRA 像素（**top-down**：负的 `biHeight` 让第 0 行就是图像顶行）。
fn dib_pixels(bitmap: HBITMAP, width: u32, height: u32) -> Option<Vec<u8>> {
    let mut info = BITMAPINFO::default();
    let header = &mut info.bmiHeader;
    header.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    header.biWidth = width as i32;
    header.biHeight = -(height as i32);
    header.biPlanes = 1;
    header.biBitCount = 32;
    header.biCompression = BI_RGB;
    header.biSizeImage = width * height * 4;

    // **必须给一个真 DC**（实测：传 NULL 时 `GetDIBits` 返回 0、像素一个都不给，
    // 而 `GetLastError` 也是 0——症状是「图标取不到」但没有任何线索）。
    // 用与屏幕兼容的内存 DC 即可，DDB → DIB 的转换不依赖它承载什么。
    let dc = DeviceContextGuard::create()?;

    let mut pixels = vec![0u8; (width as usize) * (height as usize) * 4];
    let lines = unsafe {
        GetDIBits(
            dc.0,
            bitmap,
            0,
            height,
            pixels.as_mut_ptr().cast(),
            &mut info,
            DIB_RGB_COLORS,
        )
    };
    (lines != 0).then_some(pixels)
}

/// 用 AND 掩码给「整块 alpha 都是 0」的颜色位图补透明度（掩码 0 = 不透明）。
fn apply_mask_alpha(pixels: &mut [u8], mask: &HBITMAP, width: u32, height: u32) {
    if mask.is_null() {
        return;
    }
    let Some(mask_pixels) = dib_pixels(*mask, width, height) else {
        return;
    };
    // 一个像素 4 字节：用 `as_chunks_mut` 直接拿 `[u8; 4]`（余数不可能是整像素，
    // 缓冲由 `dib_pixels` 按 width*height*4 分配）。
    let (pixels4, _remainder) = pixels.as_chunks_mut::<4>();
    for (index, pixel) in pixels4.iter_mut().enumerate() {
        let mask_pixel = mask_pixels.get(index * 4).copied().unwrap_or(0);
        pixel[3] = if mask_pixel == 0 { 255 } else { 0 };
    }
}

/// GDI 给的是 BGRA，PNG 要 RGBA：交换 R 与 B（原位）。
fn bgra_to_rgba(pixels: &mut [u8]) {
    let (pixels4, _remainder) = pixels.as_chunks_mut::<4>();
    for pixel in pixels4.iter_mut() {
        pixel.swap(0, 2);
    }
}

/// RGBA8 → `data:image/png;base64,…`。
///
/// **公开**是为了让用例能在**不依赖 Shell** 的情况下钉住契约形状（前缀 + 真 PNG 容器），
/// 见 `crates/platform/tests/icon.rs`。
pub fn data_url_from_rgba(width: u32, height: u32, rgba: &[u8]) -> Option<String> {
    if width == 0 || height == 0 || rgba.len() != (width as usize) * (height as usize) * 4 {
        return None;
    }
    let mut png_bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(rgba).ok()?;
    }
    let encoded = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
    Some(format!("{PNG_DATA_URL_PREFIX}{encoded}"))
}

/// Rust 字符串 → 以 NUL 结尾的 UTF-16（Windows 宽字符 API 的输入形态）。
fn to_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
