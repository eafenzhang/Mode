//! 切换/回滚（LT-1 C1 Task 9，`agent-core-parity.md` §7）：`MODE_AGENT_SERVER_COMMAND`
//! 指向本 Rust 二进制时的**真实拉起**。
//!
//! 跨包拿不到本包二进制的 `CARGO_BIN_EXE`（该变量只对本包集成测试可见），按控制器
//! 裁决由本包集成测试驱动 `agent-process` 的解析/spawn API：解析顺序命中 env 显式
//! 命令（`source=Env`）、子进程映像就是本二进制、进程树里没有 node/mode.cjs/tsx、
//! 启动日志带 `agent runtime=rust` 与二进制路径；`session/create` 在临时数据根
//! （基线库，与 methods.rs 同一口径）上走通；stdin EOF / kill 后无孤儿。
//!
//! 两个开关独立：这里只动命令解析 env，不碰 `MODE_HOST_INPROCESS_CHANNELS`。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use agent_process::{
    pid_is_alive, resolve_agent_command, spawn, AgentCommandSource, AgentProcessSpec,
    AgentResolveContext, AgentResolveError, RuntimeKind, SpawnedAgent,
};
use rusqlite::Connection;
use serde_json::{json, Value};

const BIN: &str = env!("CARGO_BIN_EXE_agent-server");

// ---------------------------------------------------------------------------
// 临时数据根 + 基线库（与 tests/methods.rs 同一口径：fail-closed 的 schema 门）
// ---------------------------------------------------------------------------

const LEDGER_DDL: &str = "CREATE TABLE IF NOT EXISTS schema_migration (
    id text primary key,
    checksum text not null,
    app_version text,
    time_applied integer not null
)";

fn temp_dir(label: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "agent-server-switch-{label}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("建临时目录");
    path
}

fn baseline() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("scripts")
        .join("sqlite-schema-baseline.json");
    let text = std::fs::read_to_string(&path).expect("读取 schema 基线");
    serde_json::from_str(&text).expect("schema 基线必须是 JSON")
}

fn table_ddl(name: &str, table: &Value) -> String {
    let columns = table["columns"].as_array().expect("columns 数组");
    let mut definitions = Vec::with_capacity(columns.len() + 1);
    let mut primary_key: Vec<(i64, String)> = Vec::new();
    for column in columns {
        let column_name = column["name"].as_str().expect("列名");
        let column_type = column["type"].as_str().expect("列类型");
        let mut definition = format!("{} {}", quote_identifier(column_name), column_type);
        if column["notNull"].as_bool().unwrap_or(false) {
            definition.push_str(" NOT NULL");
        }
        if let Some(default) = column["defaultValue"].as_str() {
            definition.push_str(" DEFAULT ");
            definition.push_str(default);
        }
        definitions.push(definition);
        let position = column["primaryKey"].as_i64().unwrap_or(0);
        if position > 0 {
            primary_key.push((position, column_name.to_owned()));
        }
    }
    if !primary_key.is_empty() {
        primary_key.sort_by_key(|(position, _)| *position);
        let names = primary_key
            .iter()
            .map(|(_, column)| quote_identifier(column))
            .collect::<Vec<_>>()
            .join(", ");
        definitions.push(format!("PRIMARY KEY ({names})"));
    }
    format!(
        "CREATE TABLE {} (\n  {}\n)",
        quote_identifier(name),
        definitions.join(",\n  ")
    )
}

fn quote_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// 会话库路径：`{MODE_DATA_BASE_DIR}/.mode/cli/db/db.sqlite`（`ServerConfig::db_path`）。
fn db_path(base: &Path) -> PathBuf {
    base.join(".mode").join("cli").join("db").join("db.sqlite")
}

fn create_baseline_database(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).expect("建库目录");
    let tables = baseline()["databases"]["session-store"]["tables"]
        .as_object()
        .expect("基线缺少 databases[session-store].tables")
        .clone();
    let connection = Connection::open(path).expect("创建测试库");
    for (name, table) in &tables {
        connection
            .execute_batch(&table_ddl(name, table))
            .unwrap_or_else(|error| panic!("创建表 {name} 失败：{error}"));
    }
    for (_name, table) in &tables {
        for index in table["indexes"].as_array().expect("indexes 数组") {
            let sql = index["sql"].as_str().expect("索引 SQL");
            connection
                .execute_batch(sql)
                .unwrap_or_else(|error| panic!("创建索引失败：{error}"));
        }
    }
    connection.execute_batch(LEDGER_DDL).expect("创建账本");
    connection
        .execute(
            "INSERT INTO schema_migration (id, checksum, app_version, time_applied) \
             VALUES ('0001_fixture', 'fixture-checksum', '0.0.0', 1)",
            [],
        )
        .expect("写入账本行");
}

// ---------------------------------------------------------------------------
// 解析 + 拉起：env 显式命令指向本二进制（Rust 开关）
// ---------------------------------------------------------------------------

/// 按 `MODE_AGENT_SERVER_COMMAND=<本二进制>` + `MODE_AGENT_RUNTIME=rust` 解析，
/// 断言 env 源 + Rust kind + 命令就是本二进制，返回可直接 spawn 的 spec
/// （数据根/Provider 配置 env 已注入子进程）。
fn switched_spec(base: &Path, workspace: &Path) -> AgentProcessSpec {
    let mut env = BTreeMap::new();
    env.insert("MODE_AGENT_SERVER_COMMAND".to_string(), BIN.to_string());
    env.insert("MODE_AGENT_RUNTIME".to_string(), "rust".to_string());
    let context = AgentResolveContext {
        workspace_path: workspace.to_path_buf(),
        env,
        // 临时目录向上找不到任何 dev 树：env 显式命令是唯一命中来源。
        upward_start: temp_dir("switch-upward"),
        resources_dir: None,
        node_candidates: Vec::new(),
        node_fallback: "node".into(),
        deployed_native_binary: None,
        presentation_surface: None,
        request_timeout: Duration::from_secs(60),
    };
    let resolved = resolve_agent_command(&context).expect("env 显式命令必须能解析");
    assert_eq!(resolved.source, AgentCommandSource::Env);
    assert_eq!(resolved.runtime, RuntimeKind::Rust);
    // 子进程命令就是 Rust 二进制本体（app-server --stdio）。
    assert_eq!(resolved.spec.command, BIN);
    assert_eq!(resolved.spec.args, vec!["app-server", "--stdio"]);

    let mut spec = resolved.spec;
    // 数据根指向临时目录（绝不触碰真实 ~/.mode）；Provider 配置 env 置空串
    // （空串视同未设置，中和测试宿主可能继承的值）。
    spec.env.push((
        "MODE_DATA_BASE_DIR".to_string(),
        base.to_string_lossy().into_owned(),
    ));
    spec.env.push((
        "MODE_BUILTIN_PROVIDER_CONFIG_FILE".to_string(),
        String::new(),
    ));
    spec.env.push((
        "MODE_PERSONAL_PROVIDER_CONFIG_FILE".to_string(),
        String::new(),
    ));
    spec
}

// ---------------------------------------------------------------------------
// 进程树观测（无孤儿断言）：pid → (父 pid, 映像名)
// ---------------------------------------------------------------------------

/// 快照 `(pid → (父 pid, 映像名))`；Windows 走 Toolhelp，Unix 用 ps 的 ppid 列。
#[cfg(windows)]
fn process_tree() -> HashMap<u32, (Option<u32>, String)> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    let mut tree = HashMap::new();
    let handle = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if handle == INVALID_HANDLE_VALUE {
        return tree;
    }
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..unsafe { std::mem::zeroed() }
    };
    unsafe {
        if Process32FirstW(handle, &mut entry) != 0 {
            loop {
                let length = entry
                    .szExeFile
                    .iter()
                    .position(|unit| *unit == 0)
                    .unwrap_or(entry.szExeFile.len());
                let name = String::from_utf16_lossy(&entry.szExeFile[..length]);
                tree.insert(entry.th32ProcessID, (Some(entry.th32ParentProcessID), name));
                if Process32NextW(handle, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(handle);
    }
    tree
}

#[cfg(not(windows))]
fn process_tree() -> HashMap<u32, (Option<u32>, String)> {
    let output = std::process::Command::new("ps")
        .args(["-axo", "pid=,ppid=,comm="])
        .output()
        .expect("ps 必须可用");
    let mut tree = HashMap::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut parts = line.trim().splitn(3, char::is_whitespace);
        if let (Some(pid), Some(ppid), Some(comm)) = (parts.next(), parts.next(), parts.next()) {
            if let (Ok(pid), Ok(ppid)) = (pid.parse::<u32>(), ppid.parse::<u32>()) {
                tree.insert(pid, (Some(ppid), comm.to_string()));
            }
        }
    }
    tree
}

/// 给定 pid 的全部后代映像名（父指针传递闭包；快照瞬时，仅供无孤儿断言）。
fn descendants_of(pid: u32, tree: &HashMap<u32, (Option<u32>, String)>) -> Vec<String> {
    let mut frontier = vec![pid];
    let mut seen: HashSet<u32> = HashSet::new();
    let mut names = Vec::new();
    while let Some(current) = frontier.pop() {
        for (candidate, (parent, name)) in tree {
            if *parent == Some(current) && seen.insert(*candidate) {
                frontier.push(*candidate);
                names.push(name.clone());
            }
        }
    }
    names
}

// ---------------------------------------------------------------------------
// 用例
// ---------------------------------------------------------------------------

/// 从子进程 stderr 找 `agent runtime=rust` 的启动日志（观测面，Task 9 裁决 2/3）。
async fn assert_runtime_log_line(spawned: &SpawnedAgent) {
    let mut stderr = spawned.stderr.resubscribe();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let line = tokio::time::timeout(Duration::from_secs(5), stderr.recv())
            .await
            .expect("stderr 日志超时")
            .expect("stderr 通道");
        if line.contains("agent runtime=rust") {
            assert!(
                line.contains("agent-server"),
                "启动日志必须带解析出的二进制路径，实际：{line}"
            );
            return;
        }
        assert!(Instant::now() < deadline, "未找到 runtime=rust 启动日志");
    }
}

#[tokio::test]
async fn env_command_switches_to_rust_binary_serves_create_and_exits_on_eof() {
    let base = temp_dir("switch-base");
    let workspace = temp_dir("switch-ws");
    create_baseline_database(&db_path(&base));

    let spec = switched_spec(&base, &workspace);
    let spawned = spawn(spec).await.expect("Rust agent-server 必须能启动");
    let pid = spawned.process.pid();
    assert!(pid != 0);

    // 子进程映像就是本二进制；进程树里没有 node/mode.cjs/tsx 子进程。
    let tree = process_tree();
    let child_image = tree
        .get(&pid)
        .map(|(_, name)| name.clone())
        .expect("子进程必须在快照里");
    assert!(
        child_image.to_lowercase().contains("agent-server"),
        "子进程映像应为 Rust 二进制，实际：{child_image}"
    );
    for name in descendants_of(pid, &tree) {
        let lowered = name.to_lowercase();
        assert!(
            !lowered.contains("node") && !lowered.contains("tsx") && !lowered.contains("mode.cjs"),
            "Rust runtime 不应出现 node/mode.cjs/tsx 子进程，实际：{name}"
        );
    }

    // 启动日志：`agent runtime=rust` + 二进制路径。
    assert_runtime_log_line(&spawned).await;

    // session/create 在临时基线库上走通（存储门 ready → 落行 → 快照）。
    let ws = workspace.to_string_lossy().into_owned();
    let created = spawned
        .process
        .client()
        .request(
            "session/create",
            Some(json!({
                "sessionId": "sess_switch",
                "workspace": { "workspacePath": ws, "workspaceKey": ws },
            })),
        )
        .await
        .expect("session/create 必须成功");
    assert_eq!(created["session"]["sessionId"], json!("sess_switch"));
    assert_eq!(created["session"]["title"], json!(""));

    // stdin EOF（shutdown 关写端）→ 子进程有界退出，不留孤儿。
    spawned.process.shutdown(Duration::from_secs(10)).await;
    let deadline = Instant::now() + Duration::from_secs(5);
    while pid_is_alive(pid) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!pid_is_alive(pid), "EOF 后子进程必须退出（pid {pid}）");

    std::fs::remove_dir_all(&base).ok();
    std::fs::remove_dir_all(&workspace).ok();
}

#[tokio::test]
async fn kill_reaps_switched_child_without_orphans() {
    let base = temp_dir("kill-base");
    let workspace = temp_dir("kill-ws");
    create_baseline_database(&db_path(&base));

    let spec = switched_spec(&base, &workspace);
    let spawned = spawn(spec).await.expect("Rust agent-server 必须能启动");
    let pid = spawned.process.pid();

    // 整树强杀（复用 agent-process 进程测试口径）：子进程及其后代一并回收。
    spawned.process.kill().await;
    let deadline = Instant::now() + Duration::from_secs(5);
    while pid_is_alive(pid) && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!pid_is_alive(pid), "强杀后子进程必须消失（pid {pid}）");

    // 无孤儿：快照里不再有以本 pid 为祖先的活进程。
    let tree = process_tree();
    assert!(
        descendants_of(pid, &tree).is_empty(),
        "强杀后不得残留孤儿进程"
    );

    std::fs::remove_dir_all(&base).ok();
    std::fs::remove_dir_all(&workspace).ok();
}

/// 回滚口径（解析面）：移除 env 后不再命中 Env 源。不真拉 Node——「回到 Node
/// 解析链」的归属断言由 `agent-process/tests/resolve.rs` 的 exact-source 用例
/// 覆盖；这里锁住裸环境下 Node 链走空的结果（NotConfigured）。
#[test]
fn removing_env_command_leaves_env_source() {
    let workspace = temp_dir("rollback-ws");
    let mut env = BTreeMap::new();
    env.insert("MODE_AGENT_SERVER_COMMAND".to_string(), BIN.to_string());
    let context = AgentResolveContext {
        workspace_path: workspace.clone(),
        env,
        upward_start: workspace.clone(),
        resources_dir: None,
        node_candidates: Vec::new(),
        node_fallback: "node".into(),
        deployed_native_binary: None,
        presentation_surface: None,
        request_timeout: Duration::from_secs(60),
    };
    let resolved = resolve_agent_command(&context).unwrap();
    assert_eq!(resolved.source, AgentCommandSource::Env);

    let mut rolled_back = context;
    rolled_back.env.remove("MODE_AGENT_SERVER_COMMAND");
    assert!(matches!(
        resolve_agent_command(&rolled_back),
        Err(AgentResolveError::NotConfigured { .. })
    ));
    std::fs::remove_dir_all(&workspace).ok();
}
