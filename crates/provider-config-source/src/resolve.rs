//! Node `path.resolve` 的等价实现与 `sourceKey` 计算。
//!
//! `sourceKey = sha256(resolve(activePath))`，其中 `resolve` 必须与 TS
//! `NodeModeBuiltinProviderConfigSource` 构造器里的 `path.resolve` 一致：
//! 纯词法归一，不访问文件系统、不解析符号链接、不要求路径存在。
//!
//! 支持范围：
//! - Windows 盘符绝对路径（大小写保留，`/` 归一为 `\`，折叠 `.`/`..`/重复分隔符）；
//! - Windows 相对路径拼到给定 cwd；
//! - POSIX 绝对/相对路径。
//!
//! 不保证：UNC 共享根的 `..` 越界规则、盘符相对路径（`C:foo`）。这两类不在
//! 本切片 fixture 覆盖范围内，遇到时按普通段处理。

use sha2::{Digest, Sha256};

/// 计算 `sha256(resolve(path))` 的小写 hex。
pub fn source_key_for_path(path: &str) -> String {
    let cwd = std::env::current_dir()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|_| String::from("."));
    source_key_with_cwd(path, &cwd)
}

/// 指定 cwd 的 `sha256(resolve(path))`；用于确定性测试。
pub fn source_key_with_cwd(path: &str, cwd: &str) -> String {
    sha256_hex(resolve_path_with_cwd(path, cwd).as_bytes())
}

/// `path.resolve(path)`：使用进程当前工作目录。
pub fn resolve_path(path: &str) -> String {
    let cwd = std::env::current_dir()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|_| String::from("."));
    resolve_path_with_cwd(path, &cwd)
}

/// `path.resolve(path)` 的显式 cwd 版本。
pub fn resolve_path_with_cwd(path: &str, cwd: &str) -> String {
    if cfg!(windows) {
        resolve_win32(path, cwd)
    } else {
        resolve_posix(path, cwd)
    }
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

fn resolve_win32(path: &str, cwd: &str) -> String {
    let normalized = path.replace('/', "\\");
    let combined = if is_windows_absolute(&normalized) {
        normalized
    } else {
        let cwd = cwd.replace('/', "\\");
        if normalized.is_empty() {
            cwd
        } else {
            format!("{cwd}\\{normalized}")
        }
    };
    normalize_win32(&combined)
}

fn is_windows_absolute(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\' {
        return true;
    }
    // UNC：`\\server\share`。Node 视之为绝对路径。
    value.starts_with("\\\\")
}

fn normalize_win32(value: &str) -> String {
    let (prefix, rest) = split_windows_prefix(value);
    let mut segments: Vec<&str> = Vec::new();
    for segment in rest.split('\\') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    let tail = segments.join("\\");
    match prefix {
        WindowsPrefix::Drive(drive) => {
            if tail.is_empty() {
                format!("{drive}:\\")
            } else {
                format!("{drive}:\\{tail}")
            }
        }
        WindowsPrefix::Unc(server, share) => {
            if tail.is_empty() {
                format!("\\\\{server}\\{share}")
            } else {
                format!("\\\\{server}\\{share}\\{tail}")
            }
        }
        WindowsPrefix::None => tail,
    }
}

enum WindowsPrefix<'a> {
    Drive(&'a str),
    Unc(&'a str, &'a str),
    None,
}

fn split_windows_prefix(value: &str) -> (WindowsPrefix<'_>, &str) {
    let bytes = value.as_bytes();
    if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\' {
        // 保留盘符原始大小写（Node path.resolve 不改写盘符）。
        let drive = &value[..1];
        return (WindowsPrefix::Drive(drive), &value[3..]);
    }
    if let Some(rest) = value.strip_prefix("\\\\") {
        let mut parts = rest.splitn(3, '\\');
        let server = parts.next().unwrap_or("");
        let share = parts.next().unwrap_or("");
        let tail = parts.next().unwrap_or("");
        if server.is_empty() && share.is_empty() {
            return (WindowsPrefix::None, tail);
        }
        return (WindowsPrefix::Unc(server, share), tail);
    }
    (WindowsPrefix::None, value)
}

fn resolve_posix(path: &str, cwd: &str) -> String {
    let combined = if path.starts_with('/') {
        path.to_owned()
    } else if path.is_empty() {
        cwd.to_owned()
    } else {
        format!("{}/{path}", cwd.trim_end_matches('/'))
    };
    let absolute = combined.starts_with('/');
    let mut segments: Vec<&str> = Vec::new();
    for segment in combined.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    let tail = segments.join("/");
    if absolute {
        format!("/{tail}")
    } else if tail.is_empty() {
        ".".to_owned()
    } else {
        tail
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_like_resolution() {
        // 该断言在 Windows 上才与 Node 一致；非 Windows 仅验证相对拼接。
        if cfg!(windows) {
            assert_eq!(
                resolve_path_with_cwd("C:/work/base/./provider/../active.json", "C:/ignored"),
                "C:\\work\\base\\active.json"
            );
            assert_eq!(
                resolve_path_with_cwd("provider/active.json", "C:/work/base"),
                "C:\\work\\base\\provider\\active.json"
            );
            assert_eq!(
                resolve_path_with_cwd("../active.json", "C:/work/base/sub"),
                "C:\\work\\base\\active.json"
            );
            assert_eq!(
                resolve_path_with_cwd("C:/work//base///active.json", "C:/ignored"),
                "C:\\work\\base\\active.json"
            );
            assert_eq!(
                resolve_path_with_cwd("c:/work/base/active.json", "C:/ignored"),
                "c:\\work\\base\\active.json"
            );
            assert_eq!(
                resolve_path_with_cwd("C:/work/base/provider/", "C:/ignored"),
                "C:\\work\\base\\provider"
            );
        }
    }

    #[test]
    fn source_key_is_sha256_of_resolved_path() {
        let resolved = resolve_path_with_cwd("C:/work/active.json", "C:/ignored");
        if cfg!(windows) {
            assert_eq!(
                source_key_with_cwd("C:/work/active.json", "C:/ignored"),
                sha256_hex(resolved.as_bytes())
            );
            assert_eq!(
                source_key_with_cwd("C:/work/active.json", "C:/ignored"),
                sha256_hex(b"C:\\work\\active.json")
            );
        }
    }
}
