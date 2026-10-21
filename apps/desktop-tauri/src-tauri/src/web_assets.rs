//! 把**内嵌**的前端资源解压给 Host 的 LAN Web 服务。
//!
//! 统一构建后安装包只带一份前端（`frontendDist` 内嵌进 exe）；浏览器入口 `web.html`
//! 不能由 Tauri 的 asset 协议直接提供给外部浏览器，所以在启动时把内嵌资源写到本机
//! 缓存目录，再通过 `MODE_WEB_STATIC_ROOT` 交给 Host。安装包体积不增加，磁盘上多一份
//! 解压副本（按应用版本分代，旧代启动时清理）。

use std::fs;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager, Runtime};

const WEB_ASSETS_DIR: &str = "web-assets";
const READY_MARKER: &str = ".ready";

/// 解压当前版本的内嵌前端；成功返回可作为 `MODE_WEB_STATIC_ROOT` 的目录。
///
/// 失败一律返回 `None`：桌面 UI 已由内嵌资源正常加载，LAN Web 保持 API-only，
/// 不能因为一份可选的浏览器资源让整个应用起不来。
pub fn extract<R: Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    let base = app.path().app_local_data_dir().ok()?.join(WEB_ASSETS_DIR);
    extract_to(app, &base)
}

/// `extract` 的落点可注入版本：测试用临时目录，避免写到真实的 app data。
#[doc(hidden)]
pub fn extract_to<R: Runtime>(app: &AppHandle<R>, base: &Path) -> Option<PathBuf> {
    let version = sanitize_segment(&app.package_info().version.to_string());
    let target = base.join(&version);
    if target.join(READY_MARKER).is_file() {
        return Some(target);
    }
    if fs::create_dir_all(base).is_err() {
        return None;
    }
    let temp = base.join(format!("{version}.tmp-{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp);
    if fs::create_dir_all(&temp).is_err() {
        return None;
    }

    let resolver = app.asset_resolver();
    let mut written = 0usize;
    for (path, _) in resolver.iter() {
        let Some(relative) = safe_relative_path(path.as_ref()) else {
            continue;
        };
        // 浏览器入口只认 web.html；桌面 index/about 由 Tauri 内嵌加载，不暴露给 LAN。
        if relative.as_path() == Path::new("index.html")
            || relative.as_path() == Path::new("about.html")
        {
            continue;
        }
        // tauri 默认启用 compression：`iter()` 交出的是构建期 Brotli 压缩字节，
        // 只有 `get()` 会解压。直接写 iter 的字节会让 LAN 静态服务发出损坏的资源。
        let Some(asset) = resolver.get(path.to_string()) else {
            continue;
        };
        let destination = temp.join(&relative);
        if let Some(parent) = destination.parent() {
            if fs::create_dir_all(parent).is_err() {
                let _ = fs::remove_dir_all(&temp);
                return None;
            }
        }
        if fs::write(&destination, &asset.bytes).is_err() {
            let _ = fs::remove_dir_all(&temp);
            return None;
        }
        written += 1;
    }
    if written == 0 || fs::write(temp.join(READY_MARKER), b"ok").is_err() {
        let _ = fs::remove_dir_all(&temp);
        return None;
    }

    // 先写临时目录再整体换名：中途崩溃不会留下被当作完整缓存的半份资源。
    if target.exists() {
        let _ = fs::remove_dir_all(&target);
    }
    if fs::rename(&temp, &target).is_err() {
        let _ = fs::remove_dir_all(&temp);
        return None;
    }
    prune_old_versions(base, &target);
    eprintln!("[mode-tauri] web assets extracted version={version} files={written}");
    Some(target)
}

fn sanitize_segment(raw: &str) -> String {
    let sanitized: String = raw
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}

/// 把内嵌资源的 asset key 归一化成相对路径：去掉根与空段，拒绝 `..` 与 NUL，防止解压越界。
///
/// Tauri 的 `AssetKey` 始终带根（`/assets/app.js`），所以这里不能拒绝绝对路径，只能去掉根。
fn safe_relative_path(raw: &str) -> Option<PathBuf> {
    let normalized = raw.replace('\\', "/");
    let mut relative = PathBuf::new();
    for part in normalized.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." || part.contains('\0') {
            return None;
        }
        relative.push(part);
    }
    if relative.as_os_str().is_empty() {
        None
    } else {
        Some(relative)
    }
}

/// 清理旧版本目录；只删 `web-assets` 下与当前目标不同的目录，失败忽略。
fn prune_old_versions(base: &Path, current: &Path) {
    let Ok(entries) = fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.as_path() != current && path.is_dir() {
            let _ = fs::remove_dir_all(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{safe_relative_path, sanitize_segment};
    use std::path::Path;

    #[test]
    fn relative_paths_reject_escapes() {
        assert_eq!(
            safe_relative_path("assets/app.js"),
            Some(Path::new("assets").join("app.js"))
        );
        assert_eq!(
            safe_relative_path("./web.html"),
            Some(std::path::PathBuf::from("web.html"))
        );
        assert_eq!(safe_relative_path("../secret"), None);
        assert_eq!(safe_relative_path("assets/../../secret"), None);
        assert_eq!(
            safe_relative_path("/etc/passwd"),
            Some(Path::new("etc").join("passwd"))
        );
        assert_eq!(safe_relative_path(""), None);
    }

    #[test]
    fn version_segment_is_filesystem_safe() {
        assert_eq!(sanitize_segment("0.0.73"), "0.0.73");
        assert_eq!(sanitize_segment("0.0.73 beta"), "0.0.73-beta");
        assert_eq!(sanitize_segment(""), "unknown");
    }
}
