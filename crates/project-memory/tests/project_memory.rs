//! Project Memory 服务的行为用例：目录契约、排序、精确文件名、大小上限与路径校验。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use project_memory::{MemoryError, ProjectMemoryService};

fn unique_root(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "mode-project-memory-{name}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("建临时根");
    root
}

fn write_memory(root: &Path, workspace: &str, file: &str, content: &str) {
    let dir = root.join("cli/memories/projects").join(workspace).join("memory");
    fs::create_dir_all(&dir).expect("建 memory 目录");
    fs::write(dir.join(file), content).expect("写 memory 文件");
}

#[test]
fn missing_projects_root_lists_nothing() {
    let root = unique_root("missing");
    let service = ProjectMemoryService::new(&root);
    assert!(service.list_project_memories().unwrap().is_empty());
}

#[test]
fn lists_workspaces_with_index_first_and_label_from_suffix() {
    let root = unique_root("list");
    write_memory(&root, "proj-0123456789abcdef", "b.md", "b");
    write_memory(&root, "proj-0123456789abcdef", "MEMORY.md", "index");
    write_memory(&root, "proj-0123456789abcdef", "a.md", "a");

    let service = ProjectMemoryService::new(&root);
    let workspaces = service.list_project_memories().unwrap();
    assert_eq!(workspaces.len(), 1);
    let workspace = &workspaces[0];
    assert_eq!(workspace.id, "proj-0123456789abcdef");
    assert_eq!(workspace.label, "proj");
    let names: Vec<&str> = workspace.files.iter().map(|file| file.name.as_str()).collect();
    assert_eq!(names, vec!["MEMORY.md", "a.md", "b.md"]);
    assert_eq!(workspace.files[0].kind, "index");
    assert_eq!(workspace.files[1].kind, "item");
    assert_eq!(workspace.updated_at, workspace.files.iter().map(|f| f.updated_at).max().unwrap());
}

#[test]
fn newer_workspace_sorts_first() {
    let root = unique_root("sort");
    write_memory(&root, "older-0000000000000001", "MEMORY.md", "old");
    std::thread::sleep(std::time::Duration::from_millis(30));
    write_memory(&root, "newer-0000000000000002", "MEMORY.md", "new");

    let service = ProjectMemoryService::new(&root);
    let workspaces = service.list_project_memories().unwrap();
    let ids: Vec<&str> = workspaces.iter().map(|workspace| workspace.id.as_str()).collect();
    assert_eq!(ids, vec!["newer-0000000000000002", "older-0000000000000001"]);
}

#[test]
fn reads_file_content_and_timestamp() {
    let root = unique_root("read");
    write_memory(&root, "ws-0123456789abcdef", "MEMORY.md", "# Title\nbody");
    let service = ProjectMemoryService::new(&root);
    let content = service
        .read_project_memory_file("ws-0123456789abcdef", "MEMORY.md")
        .expect("读取");
    assert_eq!(content.content, "# Title\nbody");
    assert!(content.updated_at > 0);
}

#[test]
fn rejects_invalid_segments_and_non_markdown_names() {
    let root = unique_root("invalid");
    write_memory(&root, "ws-0123456789abcdef", "MEMORY.md", "x");
    let service = ProjectMemoryService::new(&root);

    assert!(matches!(
        service.read_project_memory_file("..", "MEMORY.md"),
        Err(MemoryError::InvalidPath)
    ));
    assert!(matches!(
        service.read_project_memory_file("ws-0123456789abcdef", "../MEMORY.md"),
        Err(MemoryError::InvalidPath)
    ));
    assert!(matches!(
        service.read_project_memory_file("ws-0123456789abcdef", "notes.txt"),
        Err(MemoryError::InvalidPath)
    ));
}

#[test]
fn requires_exact_file_name_case() {
    let root = unique_root("case");
    write_memory(&root, "ws-0123456789abcdef", "MEMORY.md", "x");
    let service = ProjectMemoryService::new(&root);
    let error = service
        .read_project_memory_file("ws-0123456789abcdef", "memory.md")
        .expect_err("大小写不一致必须拒绝");
    assert!(matches!(error, MemoryError::Io(_)), "{error:?}");
}

#[test]
fn rejects_files_over_five_mebibytes() {
    let root = unique_root("limit");
    let dir = root.join("cli/memories/projects/ws-0123456789abcdef/memory");
    fs::create_dir_all(&dir).expect("建目录");
    fs::write(dir.join("MEMORY.md"), vec![b'x'; 5 * 1024 * 1024 + 16]).expect("写大文件");

    let service = ProjectMemoryService::new(&root);
    let error = service
        .read_project_memory_file("ws-0123456789abcdef", "MEMORY.md")
        .expect_err("超过 5 MiB 必须拒绝");
    assert!(matches!(error, MemoryError::PreviewLimitExceeded(_)), "{error:?}");
    assert_eq!(error.code(), Some("PROJECT_MEMORY_PREVIEW_LIMIT_EXCEEDED"));
}

#[cfg(unix)]
#[test]
fn skips_symlinked_workspaces_and_files() {
    use std::os::unix::fs::symlink;

    let root = unique_root("symlink");
    write_memory(&root, "real-0123456789abcdef", "MEMORY.md", "real");
    let projects = root.join("cli/memories/projects");
    symlink(projects.join("real-0123456789abcdef"), projects.join("link-aaaaaaaaaaaaaaaa"))
        .expect("建目录符号链接");

    let service = ProjectMemoryService::new(&root);
    let workspaces = service.list_project_memories().unwrap();
    assert_eq!(workspaces.len(), 1, "符号链接工作区必须被跳过");
    assert_eq!(workspaces[0].id, "real-0123456789abcdef");
}
