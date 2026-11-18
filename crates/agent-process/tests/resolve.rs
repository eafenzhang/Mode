//! 命令解析 → `RuntimeKind` 映射（LT-1 C1 Task 9，`agent-core-parity.md` §7 观测面）。
//!
//! exact-source 断言：env 显式命令按 `MODE_AGENT_RUNTIME` 提示（缺省 = unknown
//! origin 收敛 Node；非法值收敛 Node 并告警）；native binary 兜底是 Rust；
//! dev dist / dev 源码 / 打包 mode.cjs 都是 Node。回滚口径：移除
//! `MODE_AGENT_SERVER_COMMAND` 后回到 Node 解析链。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use agent_process::{resolve_agent_command, AgentCommandSource, AgentResolveContext, RuntimeKind};

const DEV_DIST_ENTRY: &str = "apps/mode-cli/packages/cli/dist/mode.cjs";
const DEV_SOURCE_ENTRY: &str = "apps/mode-cli/packages/cli/src/main.ts";
const PACKAGED_ENTRY: &str = "agent/glm/mode.cjs";

fn context(workspace: &Path) -> AgentResolveContext {
    AgentResolveContext {
        workspace_path: workspace.to_path_buf(),
        env: BTreeMap::new(),
        upward_start: workspace.to_path_buf(),
        resources_dir: None,
        node_candidates: Vec::new(),
        node_fallback: "node".into(),
        deployed_native_binary: None,
        presentation_surface: None,
        request_timeout: Duration::from_secs(60),
    }
}

fn temp_dir(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "mode-agent-resolve-runtime-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn env_command(ctx: &mut AgentResolveContext, command: &str) {
    ctx.env
        .insert("MODE_AGENT_SERVER_COMMAND".into(), command.into());
}

#[test]
fn hint_parser_maps_rust_node_and_defaults_to_node() {
    assert_eq!(RuntimeKind::from_hint(Some("rust")), Ok(RuntimeKind::Rust));
    assert_eq!(
        RuntimeKind::from_hint(Some(" RUST ")),
        Ok(RuntimeKind::Rust),
        "提示值大小写与空白宽容"
    );
    assert_eq!(RuntimeKind::from_hint(Some("node")), Ok(RuntimeKind::Node));
    assert_eq!(RuntimeKind::from_hint(None), Ok(RuntimeKind::Node));
    assert_eq!(RuntimeKind::from_hint(Some("")), Ok(RuntimeKind::Node));
    assert_eq!(
        RuntimeKind::from_hint(Some("bogus")),
        Err("bogus".to_string()),
        "非法提示必须显式报错，供调用方收敛 Node 并告警"
    );
}

#[test]
fn env_command_with_rust_hint_is_rust() {
    let workspace = temp_dir("env-rust");
    let mut ctx = context(&workspace);
    env_command(&mut ctx, "C:/deploy/agent-server.exe");
    ctx.env.insert("MODE_AGENT_RUNTIME".into(), "rust".into());

    let resolved = resolve_agent_command(&ctx).unwrap();
    assert_eq!(resolved.source, AgentCommandSource::Env);
    assert_eq!(resolved.runtime, RuntimeKind::Rust);
    assert_eq!(resolved.spec.command, "C:/deploy/agent-server.exe");
    std::fs::remove_dir_all(&workspace).ok();
}

#[test]
fn env_command_with_node_hint_is_node() {
    let workspace = temp_dir("env-node");
    let mut ctx = context(&workspace);
    env_command(&mut ctx, "node");
    ctx.env.insert("MODE_AGENT_RUNTIME".into(), "node".into());

    let resolved = resolve_agent_command(&ctx).unwrap();
    assert_eq!(resolved.source, AgentCommandSource::Env);
    assert_eq!(resolved.runtime, RuntimeKind::Node);
    std::fs::remove_dir_all(&workspace).ok();
}

#[test]
fn env_command_without_hint_is_node_unknown_origin() {
    let workspace = temp_dir("env-unknown");
    let mut ctx = context(&workspace);
    // env 命令可以是任何东西（包装脚本、node、原生二进制）：没有提示时按
    // unknown origin 收敛 Node（Task 9 控制器裁决 1）。
    env_command(&mut ctx, "C:/whatever/entry.cmd");

    let resolved = resolve_agent_command(&ctx).unwrap();
    assert_eq!(resolved.source, AgentCommandSource::Env);
    assert_eq!(resolved.runtime, RuntimeKind::Node);
    std::fs::remove_dir_all(&workspace).ok();
}

#[test]
fn env_command_with_invalid_hint_falls_back_to_node() {
    let workspace = temp_dir("env-invalid");
    let mut ctx = context(&workspace);
    env_command(&mut ctx, "C:/deploy/agent-server.exe");
    ctx.env.insert("MODE_AGENT_RUNTIME".into(), "bogus".into());

    // 非法提示不失败、不猜测：收敛 Node（resolve 在此分支会打 stderr 警告）。
    let resolved = resolve_agent_command(&ctx).unwrap();
    assert_eq!(resolved.source, AgentCommandSource::Env);
    assert_eq!(resolved.runtime, RuntimeKind::Node);
    assert_eq!(resolved.spec.command, "C:/deploy/agent-server.exe");
    std::fs::remove_dir_all(&workspace).ok();
}

#[test]
fn native_binary_source_is_rust() {
    let workspace = temp_dir("native-rust");
    let mut ctx = context(&workspace);
    ctx.deployed_native_binary = Some("C:/deploy/glm/mode-agent".into());

    let resolved = resolve_agent_command(&ctx).unwrap();
    assert_eq!(resolved.source, AgentCommandSource::NativeBinary);
    assert_eq!(resolved.runtime, RuntimeKind::Rust);
    std::fs::remove_dir_all(&workspace).ok();
}

#[test]
fn dev_dist_dev_source_and_packaged_are_node() {
    // dev dist：命令是 Node 解释器 + mode.cjs 入口。
    let dist_workspace = temp_dir("node-devdist");
    let entry = dist_workspace.join(DEV_DIST_ENTRY);
    std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
    std::fs::write(&entry, "").unwrap();
    let resolved = resolve_agent_command(&context(&dist_workspace)).unwrap();
    assert_eq!(resolved.source, AgentCommandSource::DevDist);
    assert_eq!(resolved.runtime, RuntimeKind::Node);
    std::fs::remove_dir_all(&dist_workspace).ok();

    // dev 源码：tsx 解释执行 main.ts。
    let source_workspace = temp_dir("node-devsource");
    let main = source_workspace.join(DEV_SOURCE_ENTRY);
    std::fs::create_dir_all(main.parent().unwrap()).unwrap();
    std::fs::write(&main, "").unwrap();
    for tsx in ["node_modules/.bin/tsx", "node_modules/.bin/tsx.cmd"] {
        let path = source_workspace.join(tsx);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "").unwrap();
    }
    let resolved = resolve_agent_command(&context(&source_workspace)).unwrap();
    assert_eq!(resolved.source, AgentCommandSource::DevSource);
    assert_eq!(resolved.runtime, RuntimeKind::Node);
    std::fs::remove_dir_all(&source_workspace).ok();

    // 打包资源：Node 解释执行 <resources>/agent/glm/mode.cjs。
    let packaged_workspace = temp_dir("node-packaged");
    let resources = temp_dir("node-packaged-res");
    let packaged = resources.join(PACKAGED_ENTRY);
    std::fs::create_dir_all(packaged.parent().unwrap()).unwrap();
    std::fs::write(&packaged, "").unwrap();
    let mut ctx = context(&packaged_workspace);
    ctx.resources_dir = Some(resources.clone());
    let resolved = resolve_agent_command(&ctx).unwrap();
    assert_eq!(resolved.source, AgentCommandSource::Packaged);
    assert_eq!(resolved.runtime, RuntimeKind::Node);
    std::fs::remove_dir_all(&packaged_workspace).ok();
    std::fs::remove_dir_all(&resources).ok();
}

#[test]
fn unsetting_env_command_rolls_back_to_node_chain() {
    let workspace = temp_dir("rollback");
    let entry = workspace.join(DEV_DIST_ENTRY);
    std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
    std::fs::write(&entry, "").unwrap();
    let node = workspace.join("node/node.exe");
    std::fs::create_dir_all(node.parent().unwrap()).unwrap();
    std::fs::write(&node, "").unwrap();

    // 开关打开：env 显式命令生效（此处以 rust 提示指向 Rust 二进制为例）。
    let mut switched = context(&workspace);
    env_command(&mut switched, "C:/deploy/agent-server.exe");
    switched
        .env
        .insert("MODE_AGENT_RUNTIME".into(), "rust".into());
    let resolved = resolve_agent_command(&switched).unwrap();
    assert_eq!(resolved.source, AgentCommandSource::Env);
    assert_eq!(resolved.runtime, RuntimeKind::Rust);

    // 回滚：移除 env（不迁移数据）→ 回到 Node 解析链（dev dist + Node 命令）。
    let mut rolled_back = context(&workspace);
    rolled_back.node_candidates = vec![node.clone()];
    let resolved = resolve_agent_command(&rolled_back).unwrap();
    assert_eq!(resolved.source, AgentCommandSource::DevDist);
    assert_eq!(resolved.runtime, RuntimeKind::Node);
    assert_eq!(resolved.spec.command, node.to_string_lossy());
    std::fs::remove_dir_all(&workspace).ok();
}
