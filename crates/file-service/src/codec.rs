//! 工作区文件条目的列式编解码（`workspaceFileEntriesCodec.ts`）。
//!
//! 每行 `type\trelativePath`；`\`、`\t`、`\n` 转义。`name` 与 `path` 由解包方还原。

use crate::types::{EntryType, WorkspaceFileEntry};

/// 单块目标大小（UTF-16 码元）；调用方按 totalLength 分块拉取。
pub const WORKSPACE_FILE_ENTRIES_CHUNK_SIZE: usize = 512 * 1024;

fn escape_field(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out
}

fn unescape_field(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

pub fn pack_entries(entries: &[WorkspaceFileEntry]) -> String {
    let mut out = String::new();
    for (index, entry) in entries.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(entry.r#type.as_str());
        out.push('\t');
        out.push_str(&escape_field(&entry.relative_path));
    }
    out
}

/// `root_path` 以分隔符结尾与否都兼容；Windows root 下把 posix 相对路径换回 `\`。
pub fn unpack_entries(packed: &str, root_path: &str) -> Vec<WorkspaceFileEntry> {
    if packed.is_empty() {
        return Vec::new();
    }
    let separator = if root_path.contains('\\') { '\\' } else { '/' };
    let prefix = if root_path.ends_with('/') || root_path.ends_with('\\') {
        root_path.to_string()
    } else {
        format!("{root_path}{separator}")
    };

    packed
        .split('\n')
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let tab = line.find('\t')?;
            let relative_path = unescape_field(&line[tab + 1..]);
            let last_slash = relative_path.rfind('/');
            let name = match last_slash {
                Some(index) => relative_path[index + 1..].to_string(),
                None => relative_path.clone(),
            };
            let path_part = if separator == '/' {
                relative_path.clone()
            } else {
                relative_path.replace('/', "\\")
            };
            let r#type = if &line[..tab] == "directory" {
                EntryType::Directory
            } else {
                EntryType::File
            };
            Some(WorkspaceFileEntry {
                name,
                path: format!("{prefix}{path_part}"),
                relative_path,
                r#type,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, rel: &str, ty: EntryType) -> WorkspaceFileEntry {
        WorkspaceFileEntry {
            name: name.into(),
            path: format!("/ws/{rel}"),
            relative_path: rel.into(),
            r#type: ty,
        }
    }

    #[test]
    fn packs_and_escapes_control_characters() {
        let entries = vec![
            entry("src", "src", EntryType::Directory),
            entry("a\tb", "src/a\tb\nc\\d", EntryType::File),
        ];
        assert_eq!(
            pack_entries(&entries),
            "directory\tsrc\nfile\tsrc/a\\tb\\nc\\\\d"
        );
    }

    #[test]
    fn unpacks_on_posix_and_windows_roots() {
        let packed = "directory\tsrc\nfile\tsrc/a.ts";
        let posix = unpack_entries(packed, "/ws");
        assert_eq!(posix[0].path, "/ws/src");
        assert_eq!(posix[1].path, "/ws/src/a.ts");
        assert_eq!(posix[1].name, "a.ts");

        let windows = unpack_entries(packed, "C:\\ws");
        assert_eq!(windows[0].path, "C:\\ws\\src");
        assert_eq!(windows[1].path, "C:\\ws\\src\\a.ts");
        assert_eq!(windows[1].relative_path, "src/a.ts");

        let trailing = unpack_entries(packed, "C:\\ws\\");
        assert_eq!(trailing[1].path, "C:\\ws\\src\\a.ts");
    }

    #[test]
    fn empty_or_malformed_lines_are_skipped() {
        assert!(unpack_entries("", "/ws").is_empty());
        assert!(unpack_entries("\n\n", "/ws").is_empty());
        assert!(unpack_entries("no-tab", "/ws").is_empty());
    }
}
