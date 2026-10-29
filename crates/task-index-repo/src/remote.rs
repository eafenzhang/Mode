//! 远端 workspace identity 解析（`parseRemoteWorkspaceIdentity` 的等价实现）。
//!
//! 行为真源：`packages/shared/src/remote-workspace-identity.ts`。
//! 这里只回答“是否合法远端 identity”，不构造/归一化 identity。

const REMOTE_IDENTITY_PREFIX: &str = "remote:";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Ssh,
    Wsl,
    Docker,
    Lan,
}

impl Kind {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "ssh" => Some(Self::Ssh),
            "wsl" => Some(Self::Wsl),
            "docker" => Some(Self::Docker),
            "lan" => Some(Self::Lan),
            _ => None,
        }
    }

    /// authority 必选段数（不含 kind）。
    fn authority_segments(self) -> usize {
        match self {
            Self::Ssh => 3,
            Self::Wsl => 1,
            Self::Docker => 1,
            Self::Lan => 2,
        }
    }
}

fn find_colon(value: &str, from: usize) -> Option<usize> {
    value.get(from..)?.find(':').map(|index| from + index)
}

fn is_drive_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes.len() == 2 || bytes[2] == b'/')
}

/// `isRemoteWorkspaceIdentity`。
pub(crate) fn is_remote_workspace_identity(identity: &str) -> bool {
    let Some(rest) = identity.strip_prefix(REMOTE_IDENTITY_PREFIX) else {
        return false;
    };
    let Some(kind_end) = rest.find(':') else {
        return false;
    };
    if kind_end == 0 {
        return false;
    }
    let Some(kind) = Kind::parse(&rest[..kind_end]) else {
        return false;
    };
    // 逐段消费 authority；path 段可能含 ":"，不能整体 split。
    let mut cursor = kind_end + 1;
    for _ in 0..kind.authority_segments() {
        let Some(next) = find_colon(rest, cursor) else {
            return false;
        };
        if next <= cursor {
            return false;
        }
        cursor = next + 1;
    }
    // WSL 可选 user 段：远端路径必须以 "/" 开头。
    if kind == Kind::Wsl && rest.as_bytes().get(cursor).copied() != Some(b'/') {
        let Some(user_end) = find_colon(rest, cursor) else {
            return false;
        };
        if user_end <= cursor {
            return false;
        }
        cursor = user_end + 1;
    }
    let workspace_path = &rest[cursor..];
    if kind == Kind::Lan {
        if is_drive_path(workspace_path) {
            return true;
        }
        if let Some(stripped) = workspace_path.strip_prefix('/') {
            return is_drive_path(stripped) || workspace_path.starts_with('/');
        }
        return false;
    }
    workspace_path.starts_with('/')
}

#[cfg(test)]
mod tests {
    use super::is_remote_workspace_identity;

    #[test]
    fn accepts_documented_shapes() {
        assert!(is_remote_workspace_identity("remote:ssh:host:22:user:/srv/ws"));
        assert!(is_remote_workspace_identity("remote:wsl:Ubuntu:/home/u/ws"));
        assert!(is_remote_workspace_identity("remote:wsl:Ubuntu:alice:/home/u/ws"));
        assert!(is_remote_workspace_identity("remote:docker:ctr:/srv/ws"));
        assert!(is_remote_workspace_identity("remote:lan:host:9000:/srv/ws"));
        assert!(is_remote_workspace_identity("remote:lan:host:9000/D:/ws"));
    }

    #[test]
    fn rejects_malformed_shapes() {
        assert!(!is_remote_workspace_identity("D:/ws"));
        assert!(!is_remote_workspace_identity("remote:"));
        assert!(!is_remote_workspace_identity("remote:ssh:host:22:user:relative"));
        assert!(!is_remote_workspace_identity("remote:wsl::/home"));
        assert!(!is_remote_workspace_identity("remote:lan:host:9000"));
    }
}
