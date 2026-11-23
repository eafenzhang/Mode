//! workspace 身份归一与 hash（W1 基元）+ 远程 workspace identity 构造/解析（16.4d 底座）。
//!
//! 真源：
//! - `packages/shared/src/remote-workspace-identity.ts`（identity 构造/解析）
//! - `packages/shared/src/remoteTarget.ts`（target 判等、去 secret、LAN 端点排序）
//! - `packages/services/src/paths.ts::getWorkspaceKey` / `getWorkspaceHash`
//!
//! 口径（AGENTS.md「Workspace Identity」）：身份 key 统一为
//! `workspaceIdentity?.trim() || workspacePath`，用于去重、绑定、缓存、队列、持久化与请求关联；
//! **实际文件操作、命令 cwd、Git 与路径展示仍用 `workspacePath`**，不要拿 hash 当路径。
//!
//! 远程 identity 契约见 `docs/specs/remote-workspace-identity.md`：构造与解析只走
//! [`build_remote_workspace_identity`] / [`parse_remote_workspace_identity`]，
//! 业务代码禁止手写拼接或拷贝归一化。

use sha2::{Digest, Sha256};

/// 与 TS 一致：反斜杠转 `/`、合并重复 `/`、去掉首尾 `/`；
/// Windows 盘符保留 `X:/` 形态，其余路径统一补一个前导 `/`（所以空串归一为 `/`）。
pub fn normalize_workspace_path_for_identity(path: &str) -> String {
    let unified = path.replace('\\', "/");
    let mut normalized = String::with_capacity(unified.len());
    let mut previous_slash = false;
    for ch in unified.chars() {
        if ch == '/' {
            if previous_slash {
                continue;
            }
            previous_slash = true;
        } else {
            previous_slash = false;
        }
        normalized.push(ch);
    }
    let trimmed = normalized.trim_matches('/');
    let is_windows_drive = {
        let bytes = trimmed.as_bytes();
        bytes.len() >= 2
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && (bytes.len() == 2 || bytes[2] == b'/')
    };
    if is_windows_drive {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    }
}

/// 身份 key：identity 去空白后非空就用它，否则用路径。
pub fn workspace_key(workspace_path: &str, workspace_identity: Option<&str>) -> String {
    match workspace_identity.map(str::trim) {
        Some(identity) if !identity.is_empty() => identity.to_string(),
        _ => workspace_path.to_string(),
    }
}

/// `workspaceKey` 的 SHA-256 前 12 位十六进制（与 Mode session 持久化一致）。
pub fn workspace_hash(key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    hex[..12].to_string()
}

/// 便捷组合：先取 key 再取 hash。
pub fn workspace_hash_for(workspace_path: &str, workspace_identity: Option<&str>) -> String {
    workspace_hash(&workspace_key(workspace_path, workspace_identity))
}

// ---------------------------------------------------------------------------
// 远程 workspace identity（`remote-workspace-identity.ts` 的 Rust 等价）
// ---------------------------------------------------------------------------

/// `RemoteTarget` 的 Rust 形态。
///
/// 只保留构造/判等 identity 需要的字段：secret（token/密码/口令）与展示字段
/// （serverName/sshConfigAlias）**不参与** identity 与判等，因此不进这个结构。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteTargetRef {
    Ssh {
        host: String,
        port: Option<u16>,
        username: String,
        private_key_path: Option<String>,
    },
    Wsl {
        distro: Option<String>,
        user: Option<String>,
    },
    Docker {
        container: String,
    },
    Lan {
        host: String,
        port: u16,
    },
    V3 {
        device_id: String,
    },
}

impl RemoteTargetRef {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Ssh { .. } => "ssh",
            Self::Wsl { .. } => "wsl",
            Self::Docker { .. } => "docker",
            Self::Lan { .. } => "lan",
            Self::V3 { .. } => "v3",
        }
    }
}

/// 解析后的远程 identity：只透出 kind 与远端真实路径（authority 对消费方无意义）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedRemoteWorkspaceIdentity {
    pub kind: &'static str,
    pub workspace_path: String,
}

const REMOTE_IDENTITY_PREFIX: &str = "remote:";

/// authority 必选段数（不含 kind）：ssh = host/port/username，其余 = 单段（lan = 2）。
fn authority_segments(kind: &str) -> Option<usize> {
    match kind {
        "ssh" => Some(3),
        "wsl" => Some(1),
        "docker" => Some(1),
        // 局域网对端：host + port；host 禁止含 ":"，否则段推进解析会失真。
        "lan" => Some(2),
        // v3 同账号 Host：authority 只有 deviceId。
        "v3" => Some(1),
        _ => None,
    }
}

/// 统一构造远程 workspace identity。Host/Main/UI 禁止自行拼接 authority；
/// `workspace_path` 只在这里归一后进入身份键，实际 IO 仍使用调用方原路径。
pub fn build_remote_workspace_identity(workspace_path: &str, target: &RemoteTargetRef) -> String {
    let normalized = normalize_workspace_path_for_identity(workspace_path);
    match target {
        RemoteTargetRef::Ssh {
            host,
            port,
            username,
            ..
        } => format!(
            "remote:ssh:{}:{}:{}:{}",
            host.trim().to_lowercase(),
            port.unwrap_or(22),
            username.trim(),
            normalized
        ),
        RemoteTargetRef::Wsl { distro, user } => {
            let distro = distro
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("default");
            match user
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                Some(user) => format!("remote:wsl:{distro}:{user}:{normalized}"),
                None => format!("remote:wsl:{distro}:{normalized}"),
            }
        }
        RemoteTargetRef::Docker { container } => {
            format!("remote:docker:{}:{normalized}", container.trim())
        }
        RemoteTargetRef::Lan { host, port } => format!(
            "remote:lan:{}:{}:{normalized}",
            host.trim().to_lowercase(),
            port
        ),
        RemoteTargetRef::V3 { device_id } => {
            format!("remote:v3:{}:{normalized}", device_id.trim())
        }
    }
}

/// 解析远程 workspace identity；非法/非远程 identity 返回 `None`（调用方回落本地路径）。
///
/// 只提取 `workspace_path`——authority 细节对消费方（CLI 运行在远端机器上）无意义。
pub fn parse_remote_workspace_identity(identity: &str) -> Option<ParsedRemoteWorkspaceIdentity> {
    let rest = identity.strip_prefix(REMOTE_IDENTITY_PREFIX)?;
    let kind_end = rest.find(':')?;
    if kind_end == 0 {
        return None;
    }
    let kind = &rest[..kind_end];
    let segments = authority_segments(kind)?;
    // 逐段消费 authority；path 段可能含 ":"（理论上 posix 路径允许），
    // 因此不能整体 split——按段推进后取剩余整段为 path。
    let mut cursor = kind_end + 1;
    for _ in 0..segments {
        let next = cursor + rest.get(cursor..)?.find(':')?;
        if next <= cursor {
            return None;
        }
        cursor = next + 1;
    }
    // WSL identity 为区分默认用户与显式用户增加了可选 user 段，旧解析器
    // 仍只消费 distro，导致 user 被误判为路径并让 identity 整体解析失败。
    // 远端路径必须以 "/" 开头，因此可以无歧义地区分 legacy 无 user 格式与显式 user 格式。
    if kind == "wsl" && !rest[cursor..].starts_with('/') {
        let user_end = cursor + rest.get(cursor..)?.find(':')?;
        if user_end <= cursor {
            return None;
        }
        cursor = user_end + 1;
    }
    let workspace_path = &rest[cursor..];
    // lan / v3 对端可能是 Windows 也可能是 posix：盘符开头与 "/" 开头都接受。
    if kind == "lan" || kind == "v3" {
        // 盘符开头（规范/野外形态）直接通过；`/D:/…` 旧规范形态归一为 `D:/…`；
        // posix 对端照旧要求绝对路径；其余一律 fail-closed。
        if is_windows_drive_path(workspace_path) {
            return Some(parsed(kind, workspace_path));
        }
        if let Some(stripped) = workspace_path.strip_prefix('/') {
            if is_windows_drive_path(stripped) {
                return Some(parsed(kind, stripped));
            }
            return Some(parsed(kind, workspace_path));
        }
        return None;
    }
    if !workspace_path.starts_with('/') {
        return None;
    }
    Some(parsed(kind, workspace_path))
}

fn parsed(kind: &str, workspace_path: &str) -> ParsedRemoteWorkspaceIdentity {
    // `kind` 已由 authority_segments 校验过，必定是已知枚举之一。
    let kind = match kind {
        "ssh" => "ssh",
        "wsl" => "wsl",
        "docker" => "docker",
        "lan" => "lan",
        _ => "v3",
    };
    ParsedRemoteWorkspaceIdentity {
        kind,
        workspace_path: workspace_path.to_string(),
    }
}

/// `X:` 或 `X:/…` 形态（Windows 盘符路径）。
fn is_windows_drive_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes.len() == 2 || bytes[2] == b'/')
}

/// identity 是否是远程 workspace identity（可被 [`parse_remote_workspace_identity`] 解析）。
pub fn is_remote_workspace_identity(identity: &str) -> bool {
    parse_remote_workspace_identity(identity).is_some()
}

/// 两个 target 是否指向同一远端（只比 authority，不比 secret 与展示字段）。
pub fn is_same_remote_target(left: &RemoteTargetRef, right: &RemoteTargetRef) -> bool {
    match (left, right) {
        (
            RemoteTargetRef::Ssh {
                host: lh,
                port: lp,
                username: lu,
                private_key_path: lk,
            },
            RemoteTargetRef::Ssh {
                host: rh,
                port: rp,
                username: ru,
                private_key_path: rk,
            },
        ) => {
            lh.trim().to_lowercase() == rh.trim().to_lowercase()
                && lp.unwrap_or(22) == rp.unwrap_or(22)
                && lu.trim() == ru.trim()
                && normalize_opt_key(lk.as_deref()) == normalize_opt_key(rk.as_deref())
        }
        (
            RemoteTargetRef::Wsl {
                distro: ld,
                user: lu,
            },
            RemoteTargetRef::Wsl {
                distro: rd,
                user: ru,
            },
        ) => {
            normalize_distro(ld.as_deref()) == normalize_distro(rd.as_deref())
                && normalize_opt_key(lu.as_deref()) == normalize_opt_key(ru.as_deref())
        }
        (RemoteTargetRef::Docker { container: lc }, RemoteTargetRef::Docker { container: rc }) => {
            lc.trim() == rc.trim()
        }
        (
            RemoteTargetRef::Lan { host: lh, port: lp },
            RemoteTargetRef::Lan { host: rh, port: rp },
        ) => lh.trim().to_lowercase() == rh.trim().to_lowercase() && lp == rp,
        (RemoteTargetRef::V3 { device_id: ld }, RemoteTargetRef::V3 { device_id: rd }) => ld == rd,
        _ => false,
    }
}

fn normalize_opt_key(value: Option<&str>) -> String {
    value.map(str::trim).unwrap_or("").to_string()
}

fn normalize_distro(value: Option<&str>) -> String {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        Some(distro) => distro.to_string(),
        None => "default".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_backslashes_and_duplicate_slashes() {
        assert_eq!(
            normalize_workspace_path_for_identity("D:\\\\ZCodium\\\\"),
            "D:/ZCodium"
        );
        assert_eq!(
            normalize_workspace_path_for_identity("/home//user/"),
            "/home/user"
        );
        assert_eq!(
            normalize_workspace_path_for_identity("relative/path"),
            "/relative/path"
        );
        assert_eq!(normalize_workspace_path_for_identity(""), "/");
        assert_eq!(normalize_workspace_path_for_identity("//D:/x"), "D:/x");
        assert_eq!(normalize_workspace_path_for_identity("C:"), "C:");
    }

    #[test]
    fn identity_wins_over_path_and_is_trimmed() {
        assert_eq!(
            workspace_key("D:/w", Some(" remote:lan:1:D:/w ")),
            "remote:lan:1:D:/w"
        );
        assert_eq!(workspace_key("D:/w", Some("   ")), "D:/w");
        assert_eq!(workspace_key("D:/w", None), "D:/w");
    }

    fn lan_target(host: &str, port: u16) -> RemoteTargetRef {
        RemoteTargetRef::Lan {
            host: host.to_string(),
            port,
        }
    }

    #[test]
    fn builds_lan_identity_in_the_documented_shape() {
        // spec 验收 1：反斜杠 + 尾斜杠归一，host 小写。
        let identity =
            build_remote_workspace_identity("D:\\Work\\Demo\\", &lan_target("192.168.1.20", 45880));
        assert_eq!(identity, "remote:lan:192.168.1.20:45880:D:/Work/Demo");
        assert_eq!(
            parse_remote_workspace_identity(&identity),
            Some(ParsedRemoteWorkspaceIdentity {
                kind: "lan",
                workspace_path: "D:/Work/Demo".to_string(),
            })
        );
    }

    #[test]
    fn parses_wild_windows_lan_identity() {
        // spec 验收 2：野外形态（盘符开头）此前被判 null，即本次故障。
        assert_eq!(
            parse_remote_workspace_identity("remote:lan:100.68.49.120:45880:D:/gas-hub-xp"),
            Some(ParsedRemoteWorkspaceIdentity {
                kind: "lan",
                workspace_path: "D:/gas-hub-xp".to_string(),
            })
        );
    }

    #[test]
    fn parses_legacy_leading_slash_windows_lan_identity() {
        // spec 验收 3：旧规范形态 /D:/… 归一为 D:/…
        assert_eq!(
            parse_remote_workspace_identity("remote:lan:h:1:/D:/Work/Demo"),
            Some(ParsedRemoteWorkspaceIdentity {
                kind: "lan",
                workspace_path: "D:/Work/Demo".to_string(),
            })
        );
    }

    #[test]
    fn posix_lan_and_relative_paths() {
        // spec 验收 4：posix lan 不变；相对路径/缺段/非远程一律拒绝。
        assert_eq!(
            parse_remote_workspace_identity("remote:lan:h:1:/home/dev/proj")
                .map(|parsed| parsed.workspace_path),
            Some("/home/dev/proj".to_string())
        );
        assert_eq!(parse_remote_workspace_identity("remote:lan:h:1:x"), None);
        assert_eq!(parse_remote_workspace_identity("remote:lan:h"), None);
        assert_eq!(parse_remote_workspace_identity("local:/x"), None);
        assert_eq!(parse_remote_workspace_identity("remote:nope:a:b:/x"), None);
    }

    #[test]
    fn ssh_wsl_docker_keep_posix_rules() {
        let ssh = build_remote_workspace_identity(
            "/srv/app",
            &RemoteTargetRef::Ssh {
                host: "Build-01".to_string(),
                port: None,
                username: "dev".to_string(),
                private_key_path: None,
            },
        );
        assert_eq!(ssh, "remote:ssh:build-01:22:dev:/srv/app");

        // wsl：无 user 段与有 user 段都要能解析回来。
        let wsl_default = build_remote_workspace_identity(
            "/work",
            &RemoteTargetRef::Wsl {
                distro: None,
                user: None,
            },
        );
        assert_eq!(wsl_default, "remote:wsl:default:/work");
        let wsl_user = build_remote_workspace_identity(
            "/work",
            &RemoteTargetRef::Wsl {
                distro: Some("Ubuntu".to_string()),
                user: Some("dev".to_string()),
            },
        );
        assert_eq!(wsl_user, "remote:wsl:Ubuntu:dev:/work");
        assert_eq!(
            parse_remote_workspace_identity(&wsl_user).map(|parsed| parsed.workspace_path),
            Some("/work".to_string())
        );

        // docker/ssh 的 posix 强制：相对路径 fail-closed。
        assert_eq!(
            parse_remote_workspace_identity("remote:docker:box:rel"),
            None
        );
        assert_eq!(
            parse_remote_workspace_identity("remote:ssh:h:22:u:rel"),
            None
        );
    }

    #[test]
    fn same_target_ignores_secrets_and_display_fields() {
        let a = lan_target("10.0.0.5", 45879);
        let b = lan_target("  10.0.0.5 ", 45879);
        assert!(is_same_remote_target(&a, &b));
        assert!(!is_same_remote_target(&a, &lan_target("10.0.0.5", 45880)));

        // ssh：port 缺省按 22；privateKeyPath 参与判等（同 host 不同密钥是不同信任边界）。
        let ssh_a = RemoteTargetRef::Ssh {
            host: "H".to_string(),
            port: None,
            username: "u".to_string(),
            private_key_path: Some("/k".to_string()),
        };
        let ssh_b = RemoteTargetRef::Ssh {
            host: "h".to_string(),
            port: Some(22),
            username: "u".to_string(),
            private_key_path: Some(" /k ".to_string()),
        };
        assert!(is_same_remote_target(&ssh_a, &ssh_b));
        let ssh_c = RemoteTargetRef::Ssh {
            host: "h".to_string(),
            port: Some(22),
            username: "u".to_string(),
            private_key_path: Some("/other".to_string()),
        };
        assert!(!is_same_remote_target(&ssh_a, &ssh_c));

        // 跨 kind 恒 false。
        assert!(!is_same_remote_target(
            &a,
            &RemoteTargetRef::Docker {
                container: "box".to_string()
            }
        ));
    }

    #[test]
    fn hash_matches_mode_session_convention() {
        // 期望值由 Node `createHash("sha256").update(key).digest("hex").slice(0,12)` 生成。
        assert_eq!(workspace_hash("D:/ZCodium"), "67dc4aec24f7");
        assert_eq!(workspace_hash("/home/user/project"), "9dad1e4e08b0");
        assert_eq!(workspace_hash("/"), "8a5edab28263");
        assert_eq!(
            workspace_hash("remote:lan:100.90.96.50:45880:D:/ZCodium"),
            "e2bfc498d523"
        );
        assert_eq!(
            workspace_hash_for("D:/ignored", Some("D:/ZCodium")),
            "67dc4aec24f7"
        );
    }
}
