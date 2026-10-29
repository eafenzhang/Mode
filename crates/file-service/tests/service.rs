//! `file` 服务行为测试（索引/搜索的金标准在 `parity.rs`）。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use file_service::{
    CheckFilesExistParams, FileRangeParams, FileService, FileServiceOptions, IgnoreTransformParams,
    PathParams, PreviewParams, ReaddirParams, ReadTextFileParams, RootParams, ScratchWorkspaceParams,
    WorkspaceFileSearchParams,
};

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mode-file-service-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn service(root: &Path) -> FileService {
    FileService::new(FileServiceOptions {
        data_root: root.to_path_buf(),
        home: root.to_path_buf(),
        on_warn: None,
    })
}

fn path_of(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

async fn run<F: std::future::Future>(future: F) -> F::Output {
    future.await
}

#[tokio::test]
async fn readdir_hides_dotfiles_and_orders_directories_first() {
    let root = TempRoot::new("readdir");
    fs::create_dir(root.path().join("b-dir")).unwrap();
    fs::write(root.path().join("a.txt"), "").unwrap();
    fs::write(root.path().join(".env"), "").unwrap();

    let service = service(root.path());
    let entries = service
        .readdir(&ReaddirParams {
            path: path_of(root.path()),
            include_hidden: None,
        })
        .await
        .unwrap();
    assert_eq!(entries.len(), 2, "默认隐藏 dotfiles");
    assert_eq!(entries[0].name, "b-dir");
    assert_eq!(entries[0].r#type.as_str(), "directory");
    assert_eq!(entries[1].name, "a.txt");

    let with_hidden = service
        .readdir(&ReaddirParams {
            path: path_of(root.path()),
            include_hidden: Some(true),
        })
        .await
        .unwrap();
    assert!(with_hidden.iter().any(|entry| entry.name == ".env"));
}

#[tokio::test]
async fn stat_reports_directory_without_size_and_file_with_size() {
    let root = TempRoot::new("stat");
    fs::write(root.path().join("a.txt"), "hello").unwrap();
    let service = service(root.path());

    let directory = run(service.stat(&PathParams {
        path: path_of(root.path()),
    }))
    .await
    .unwrap();
    assert_eq!(directory["type"], "directory");
    assert!(directory.get("size").is_none());

    let file = run(service.stat(&PathParams {
        path: path_of(&root.path().join("a.txt")),
    }))
    .await
    .unwrap();
    assert_eq!(file["type"], "file");
    assert_eq!(file["size"], 5);
    assert!(file["mtimeMs"].as_f64().unwrap() > 0.0);
}

#[tokio::test]
async fn read_text_file_handles_text_binary_offsets_and_errors() {
    let root = TempRoot::new("read-text");
    fs::write(root.path().join("a.txt"), "hello world").unwrap();
    fs::write(root.path().join("bin.dat"), [0u8, 1, 2, 3]).unwrap();
    let service = service(root.path());

    let slice = run(service.read_text_file(&ReadTextFileParams {
        path: path_of(&root.path().join("a.txt")),
        offset: None,
        length: None,
    }))
    .await
    .unwrap();
    assert_eq!(slice.content, "hello world");
    assert_eq!(slice.bytes_read, 11);
    assert_eq!(slice.total_bytes, 11);
    assert!(!slice.is_binary);
    assert!(!slice.truncated);

    let tail = run(service.read_text_file(&ReadTextFileParams {
        path: path_of(&root.path().join("a.txt")),
        offset: Some(6.0),
        length: Some(5.0),
    }))
    .await
    .unwrap();
    assert_eq!(tail.content, "world");
    assert_eq!(tail.offset, 6);
    assert!(!tail.truncated);

    let beyond = run(service.read_text_file(&ReadTextFileParams {
        path: path_of(&root.path().join("a.txt")),
        offset: Some(99.0),
        length: None,
    }))
    .await
    .unwrap();
    assert_eq!(beyond.content, "");
    assert_eq!(beyond.bytes_read, 0);

    let binary = run(service.read_text_file(&ReadTextFileParams {
        path: path_of(&root.path().join("bin.dat")),
        offset: None,
        length: None,
    }))
    .await
    .unwrap();
    assert!(binary.is_binary);
    assert_eq!(binary.content, "");

    let error = run(service.read_text_file(&ReadTextFileParams {
        path: path_of(root.path()),
        offset: None,
        length: None,
    }))
    .await
    .unwrap_err();
    assert!(error.message.starts_with("Path is not a file:"));
}

#[tokio::test]
async fn read_file_range_returns_raw_bytes() {
    let root = TempRoot::new("read-range");
    fs::write(root.path().join("a.bin"), vec![1u8, 2, 3, 4, 5]).unwrap();
    let service = service(root.path());

    let bytes = run(service.read_file_range(&FileRangeParams {
        path: path_of(&root.path().join("a.bin")),
        offset: 1.0,
        length: 2.0,
    }))
    .await
    .unwrap();
    assert_eq!(bytes, vec![2, 3]);

    let eof = run(service.read_file_range(&FileRangeParams {
        path: path_of(&root.path().join("a.bin")),
        offset: 10.0,
        length: 2.0,
    }))
    .await
    .unwrap();
    assert!(eof.is_empty());
}

#[tokio::test]
async fn previews_base64_and_size_limits() {
    let root = TempRoot::new("preview");
    fs::write(root.path().join("a.png"), [0u8, 1, 2]).unwrap();
    fs::write(root.path().join("big.bin"), vec![0u8; 32]).unwrap();
    let service = service(root.path());

    let media = service
        .read_media_preview(&PreviewParams {
            path: path_of(&root.path().join("a.png")),
            max_bytes: None,
        })
        .await
        .unwrap();
    assert_eq!(media.media_type, "image/png");
    assert_eq!(media.data_base64, "AAEC");
    assert_eq!(media.total_bytes, 3);

    let binary = service
        .read_binary_preview(&PreviewParams {
            path: path_of(&root.path().join("big.bin")),
            max_bytes: Some(16.0),
        })
        .await
        .unwrap_err();
    assert!(binary.message.starts_with("File is too large to preview:"));

    let not_a_file = service
        .read_media_preview(&PreviewParams {
            path: path_of(root.path()),
            max_bytes: None,
        })
        .await
        .unwrap_err();
    assert!(not_a_file.message.starts_with("Path is not a file:"));
}

#[tokio::test]
async fn workspaces_create_idempotently_and_validate_names() {
    let root = TempRoot::new("workspaces");
    let service = service(root.path());

    let conversation = service.ensure_conversation_workspace().await.unwrap();
    assert!(conversation.created);
    assert_eq!(conversation.workspace_purpose, "conversation");
    assert!(Path::new(&conversation.path).is_dir());
    let again = service.ensure_conversation_workspace().await.unwrap();
    assert!(!again.created);
    assert_eq!(again.path, conversation.path);

    let default = service.create_default_workspace().await.unwrap();
    assert!(Path::new(&default.path).is_dir());

    let scratch = service
        .create_scratch_workspace(&ScratchWorkspaceParams {
            name: "  demo  ".into(),
        })
        .await
        .unwrap();
    assert!(scratch.path.ends_with("demo"));
    assert!(Path::new(&scratch.path).is_dir());

    let empty = service
        .create_scratch_workspace(&ScratchWorkspaceParams { name: "   ".into() })
        .await
        .unwrap_err();
    assert_eq!(empty.message, "Workspace name is required.");

    let separator = service
        .create_scratch_workspace(&ScratchWorkspaceParams {
            name: "a/b".into(),
        })
        .await
        .unwrap_err();
    assert_eq!(
        separator.message,
        "Workspace name cannot contain path separators."
    );
}

#[tokio::test]
async fn existence_checks_are_batched_and_cached() {
    let root = TempRoot::new("exists");
    let file = root.path().join("a.txt");
    fs::write(&file, "").unwrap();
    let service = service(root.path());

    let results = service
        .check_files_exist(&CheckFilesExistParams {
            paths: vec![path_of(&file), path_of(&root.path().join("missing.txt"))],
        })
        .await
        .unwrap();
    assert!(results[0].exists);
    assert!(!results[1].exists);

    // 60s 缓存：删除文件后同一路径仍返回 true（与 TS 缓存语义一致）。
    fs::remove_file(&file).unwrap();
    let cached = service
        .check_files_exist(&CheckFilesExistParams {
            paths: vec![path_of(&file)],
        })
        .await
        .unwrap();
    assert!(cached[0].exists);

    let too_many = service
        .check_files_exist(&CheckFilesExistParams {
            paths: (0..16).map(|index| format!("p{index}")).collect(),
        })
        .await
        .unwrap_err();
    assert_eq!(
        too_many.message,
        "File existence check supports at most 15 paths."
    );
}

#[tokio::test]
async fn resolve_path_is_canonical_without_verbatim_prefix() {
    let root = TempRoot::new("resolve");
    fs::create_dir(root.path().join("sub")).unwrap();
    let service = service(root.path());
    let messy = root.path().join("sub").join("..").join("sub");
    let resolved = service
        .resolve_path(&PathParams {
            path: path_of(&messy),
        })
        .await
        .unwrap();
    assert!(!resolved.starts_with(r"\\?\"), "{resolved}");
    let expected = fs::canonicalize(root.path().join("sub"))
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let expected = expected
        .strip_prefix(r"\\?\")
        .map(str::to_string)
        .unwrap_or(expected);
    assert_eq!(resolved, expected);
}

#[tokio::test]
async fn search_and_ignore_methods_work_through_service() {
    let root = TempRoot::new("search");
    fs::create_dir(root.path().join("src")).unwrap();
    fs::write(root.path().join("src/main.ts"), "").unwrap();
    fs::write(root.path().join("src/other.ts"), "").unwrap();
    fs::write(
        root.path().join(file_service::WORKSPACE_FILE_SEARCH_IGNORE_FILE_NAME),
        "other.ts\n",
    )
    .unwrap();
    let service = service(root.path());
    let root_path = path_of(root.path());

    let results = service
        .search_workspace_files(&WorkspaceFileSearchParams {
            root_path: root_path.clone(),
            workspace_identity: None,
            query: "main".into(),
            limit: None,
            refresh: None,
        })
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].relative_path, "src/main.ts");

    let none = service
        .search_workspace_files(&WorkspaceFileSearchParams {
            root_path: root_path.clone(),
            workspace_identity: None,
            query: "main".into(),
            limit: Some(0.0),
            refresh: None,
        })
        .await
        .unwrap();
    assert!(none.is_empty());

    let ignore = service
        .read_workspace_file_search_ignore(&RootParams {
            root_path: root_path.clone(),
        })
        .await
        .unwrap();
    assert_eq!(ignore.source, "file");
    assert_eq!(ignore.content, "other.ts\n");

    let transformed = service
        .apply_workspace_file_search_ignore_transform(&IgnoreTransformParams {
            root_path: root_path.clone(),
            transform: "reset-defaults".into(),
        })
        .await
        .unwrap();
    assert!(transformed.content.contains("node_modules/"));
}
