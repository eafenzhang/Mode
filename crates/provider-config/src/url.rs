//! `z.string().url()` 的保守近似。
//!
//! zod 委托 JS `URL` 构造器：scheme 合法、特殊 scheme 必须有 host、端口范围有限。
//! 这里只做静态可接受性判断，不做规范化/解析结果输出。

const SPECIAL_SCHEMES: [&str; 6] = ["http", "https", "ftp", "ws", "wss", "file"];

pub fn is_valid_url(value: &str) -> bool {
    // JS URL 会裁剪首尾 C0 控制字符与空格。
    let value = value.trim_matches(|c: char| c <= ' ' || c == '\u{7f}');
    if value.is_empty() {
        return false;
    }

    let Some(colon) = value.find(':') else {
        return false;
    };
    let scheme = &value[..colon];
    if !is_valid_scheme(scheme) {
        return false;
    }
    let rest = &value[colon + 1..];
    let scheme = scheme.to_ascii_lowercase();

    if SPECIAL_SCHEMES.contains(&scheme.as_str()) {
        if scheme == "file" {
            // file: 允许空 host（file:///tmp/x）。
            let authority = rest.strip_prefix("//").unwrap_or("");
            let authority = authority.split(['/', '?', '#']).next().unwrap_or("");
            return !authority.chars().any(|c| c <= ' ' || c == '\\');
        }
        let Some(authority_and_rest) = rest.strip_prefix("//") else {
            // `http:foo` 在 JS 中等价于 http://foo/。
            let first = rest.split(['/', '?', '#']).next().unwrap_or("");
            return !first.is_empty() && !first.chars().any(|c| c <= ' ');
        };
        return is_valid_authority(authority_and_rest);
    }

    // 非特殊 scheme：`a:b` / `x:` / `mailto:x@y` 都合法；只要 scheme 合法且不含控制字符。
    !rest.chars().any(|c| c < ' ' || c == '\u{7f}')
}

fn is_valid_scheme(scheme: &str) -> bool {
    let mut chars = scheme.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

fn is_valid_authority(authority_and_rest: &str) -> bool {
    let authority = authority_and_rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("");
    if authority.is_empty() {
        return false;
    }
    if authority.chars().any(|c| c <= ' ' || c == '\\') {
        return false;
    }

    // 去掉 userinfo（最后一个 @）。
    let host_port = match authority.rfind('@') {
        Some(index) => &authority[index + 1..],
        None => authority,
    };
    if host_port.is_empty() {
        return false;
    }

    if let Some(rest) = host_port.strip_prefix('[') {
        let Some(close) = rest.find(']') else {
            return false;
        };
        let after = &rest[close + 1..];
        if after.is_empty() {
            return true;
        }
        return after
            .strip_prefix(':')
            .is_some_and(is_valid_port);
    }

    // IPv4/域名 + 可选端口；host 必须非空。
    match host_port.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => {
            !host.is_empty() && is_valid_port(port)
        }
        _ => !host_port.is_empty() && !host_port.contains(':'),
    }
}

fn is_valid_port(port: &str) -> bool {
    if port.is_empty() {
        return true;
    }
    if !port.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    port.parse::<u32>().is_ok_and(|value| value <= 65_535)
}
