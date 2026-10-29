//! `commands` 服务的目录优先级、enable override、写/改/删与插件命令。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use commands_service::{
    CommandAgentSource, CommandConfig, CommandCreateParams, CommandDeleteParams,
    CommandSetEnabledParams, CommandStorageLevel, CommandUpdateParams, CommandsService,
    CommandsServiceOptions, ListParams, PrimaryDirectoryParams,
};

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mode-commands-{label}-{}-{}",
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

fn service(home: &Path) -> CommandsService {
    let mut options = CommandsServiceOptions::with_home(home);
    options.write_options.use_file_lock = false;
    CommandsService::new(options)
}

/// 按 `/` 分段拼接，保证结果与 `Path::join` 在 Windows 上产生同一串反斜杠
/// （override 的键是路径字符串，混合分隔符会导致查不到）。
fn p(base: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .filter(|segment| !segment.is_empty())
        .fold(base.to_path_buf(), |path, segment| path.join(segment))
}

fn write(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

fn write_config(home: &Path, config: serde_json::Value) {
    write(
        &home.join(".mode").join("cli").join("config.json"),
        &serde_json::to_string_pretty(&config).unwrap(),
    );
}

fn config(name: &str, prompt: &str) -> CommandConfig {
    CommandConfig {
        name: name.to_string(),
        prompt: prompt.to_string(),
        description: None,
        argument_hint: None,
        file_path: None,
    }
}

fn list(service: &CommandsService, workspace: Option<&Path>) -> commands_service::CommandsListResult {
    service
        .list(&ListParams {
            agent_source: None,
            workspace_path: workspace.map(|path| path.to_string_lossy().into_owned()),
            workspace_identity: None,
        })
        .unwrap()
}

fn names(commands: &[commands_service::UserCommand]) -> Vec<String> {
    commands.iter().map(|command| command.name.clone()).collect()
}

// ============================================================================
// 目录来源优先级
// ============================================================================

#[test]
fn workspace_mode_has_priority_over_workspace_agents() {
    let root = TempRoot::new("priority");
    let home = root.path().join("home");
    let workspace = root.path().join("ws");
    write(&workspace.join(".mode/commands/a.md"), "mode body");
    write(&workspace.join(".agents/commands/b.md"), "agents body");
    write(&home.join(".mode/commands/c.md"), "user mode");
    write(&home.join(".agents/commands/d.md"), "user agents");

    let service = service(&home);
    let result = list(&service, Some(&workspace));

    // workspace `.mode` 有命令 → workspace `.agents` 整段跳过；user 侧同理。
    assert_eq!(names(&result.user_commands), vec!["/a", "/c"]);
    assert_eq!(
        result.user_commands[0].scope,
        commands_service::CommandScope::Project
    );
    assert_eq!(
        result.user_commands[0].location.source,
        commands_service::SettingsDirectorySource::Mode
    );
    assert_eq!(result.user_commands[0].id, "modeAgent:mode:project:/a");
    assert_eq!(result.user_commands[1].id, "modeAgent:mode:global:/c");
    assert_eq!(
        result.user_commands[1].location.project_path, None
    );
    assert!(result.capability.user_scope_available);
    assert_eq!(result.commands.len(), 2);
}

#[test]
fn workspace_agents_is_used_when_mode_has_no_commands() {
    let root = TempRoot::new("agents-fallback");
    let home = root.path().join("home");
    let workspace = root.path().join("ws");
    fs::create_dir_all(workspace.join(".mode/commands")).unwrap();
    write(&workspace.join(".agents/commands/nested/b.md"), "body");

    let service = service(&home);
    let result = list(&service, Some(&workspace));

    assert_eq!(names(&result.user_commands), vec!["/nested/b"]);
    assert_eq!(
        result.user_commands[0].location.source,
        commands_service::SettingsDirectorySource::Agents
    );
    assert_eq!(result.user_commands[0].id, "modeAgent:agents:project:/nested/b");
}

#[test]
fn project_commands_come_first_and_dedupe_keeps_first_name() {
    let root = TempRoot::new("dedupe");
    let home = root.path().join("home");
    let workspace = root.path().join("ws");
    write(&workspace.join(".mode/commands/same.md"), "project");
    write(&home.join(".mode/commands/same.md"), "global");
    write(&home.join(".mode/commands/other.md"), "other");

    let service = service(&home);
    let result = list(&service, Some(&workspace));

    assert_eq!(names(&result.user_commands), vec!["/same", "/other"]);
    assert_eq!(result.user_commands[0].prompt, "project");
    assert_eq!(
        result.user_commands[0].scope,
        commands_service::CommandScope::Project
    );
}

#[test]
fn hidden_entries_and_non_markdown_files_are_skipped() {
    let root = TempRoot::new("hidden");
    let home = root.path().join("home");
    write(&home.join(".mode/commands/.hidden.md"), "hidden");
    write(&home.join(".mode/commands/ok.md"), "ok");
    write(&home.join(".mode/commands/notes.txt"), "ignored");

    let service = service(&home);
    let result = list(&service, None);
    assert_eq!(names(&result.user_commands), vec!["/ok"]);
}

#[test]
fn unreadable_missing_workspace_is_empty() {
    let root = TempRoot::new("missing");
    let home = root.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let service = service(&home);
    let result = list(&service, Some(&root.path().join("does-not-exist")));
    assert!(result.user_commands.is_empty());
}

// ============================================================================
// enable override
// ============================================================================

#[test]
fn enable_override_defaults_true_and_disables_command() {
    let root = TempRoot::new("override");
    let home = root.path().join("home");
    let file = p(&home, ".mode/commands/a.md");
    write(&file, "body");
    let file_key = file.to_string_lossy().into_owned();

    let service = service(&home);
    assert!(list(&service, None).user_commands[0].enabled);

    write_config(
        &home,
        serde_json::json!({ "theme": "dark", "command": { (file_key.clone()): { "enable": false } } }),
    );
    let result = list(&service, None);
    assert!(!result.user_commands[0].enabled);

    // enable=true 删除整条，而不是写 {enable:true}。
    service
        .set_command_enabled(&CommandSetEnabledParams {
            command_id: None,
            file_path: file_key.clone(),
            enabled: true,
            agent_source: None,
        })
        .unwrap();
    let persisted: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(home.join(".mode/cli/config.json")).unwrap())
            .unwrap();
    assert_eq!(persisted.get("theme").unwrap(), &serde_json::json!("dark"));
    assert!(persisted.get("command").is_none());
    assert!(list(&service, None).user_commands[0].enabled);
}

// ============================================================================
// 写 / 改 / 删
// ============================================================================

#[test]
fn write_update_and_delete_round_trip() {
    let root = TempRoot::new("write");
    let home = root.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let service = service(&home);

    let mut create_config = config("/hello", "say hello");
    create_config.description = Some("  greet  ".into());
    create_config.argument_hint = Some("[name]".into());
    let written = service
        .write_command_file(&CommandCreateParams {
            config: create_config,
            agent_source: None,
            storage_level: None,
            workspace_path: None,
        })
        .unwrap();

    let file = p(&home, ".mode/commands/hello.md");
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "---\ndescription: greet\nargument-hint: [name]\n---\n\nsay hello"
    );
    assert_eq!(written.command.name, "/hello");
    assert_eq!(written.command.id, "modeAgent:mode:global:/hello");
    assert!(written.command.enabled);
    assert_eq!(written.command.agent_source, CommandAgentSource::ModeAgent);
    assert_eq!(
        written.command.location.directory_path,
        p(&home, ".mode/commands").to_string_lossy()
    );

    // 重复写同一名字报错。
    let error = service
        .write_command_file(&CommandCreateParams {
            config: config("/hello", "again"),
            agent_source: None,
            storage_level: None,
            workspace_path: None,
        })
        .unwrap_err();
    assert_eq!(error.message, "Command file already exists: hello.md");

    // 先禁用，再改名：override 必须迁移到新路径。
    let old_key = file.to_string_lossy().into_owned();
    service
        .set_command_enabled(&CommandSetEnabledParams {
            command_id: None,
            file_path: old_key.clone(),
            enabled: false,
            agent_source: None,
        })
        .unwrap();

    let renamed = service
        .update_command_file(&CommandUpdateParams {
            command_id: None,
            config: config("/renamed", "renamed body"),
            agent_source: None,
            old_file_path: Some(old_key.clone()),
            storage_level: None,
            workspace_path: None,
        })
        .unwrap();

    assert_eq!(renamed.command.name, "/renamed");
    assert!(!renamed.command.enabled, "禁用状态必须跟着改名迁移");
    assert!(!file.exists());
    let new_file = p(&home, ".mode/commands/renamed.md");
    assert_eq!(fs::read_to_string(&new_file).unwrap(), "renamed body");

    service
        .delete_command_file(&CommandDeleteParams {
            command_id: None,
            file_path: new_file.to_string_lossy().into_owned(),
            agent_source: None,
        })
        .unwrap();
    assert!(!new_file.exists());

    // 删除不存在的文件不报错；override 条目被清掉。
    service
        .delete_command_file(&CommandDeleteParams {
            command_id: None,
            file_path: new_file.to_string_lossy().into_owned(),
            agent_source: None,
        })
        .unwrap();
    let persisted: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(home.join(".mode/cli/config.json")).unwrap())
            .unwrap();
    assert!(persisted.get("command").is_none());
}

#[test]
fn update_keeps_unknown_frontmatter_and_rejects_existing_target() {
    let root = TempRoot::new("update");
    let home = root.path().join("home");
    write(
        &p(&home, ".mode/commands/a.md"),
        "---\nallowed-tools: Bash\ndescription: old\n---\n\nold body",
    );
    write(&p(&home, ".mode/commands/b.md"), "b body");
    let service = service(&home);

    let updated = service
        .update_command_file(&CommandUpdateParams {
            command_id: None,
            config: config("/a", "new body"),
            agent_source: None,
            old_file_path: Some(p(&home, ".mode/commands/a.md").to_string_lossy().into_owned()),
            storage_level: None,
            workspace_path: None,
        })
        .unwrap();
    assert_eq!(updated.command.name, "/a");
    assert_eq!(
        fs::read_to_string(p(&home, ".mode/commands/a.md")).unwrap(),
        "---\nallowed-tools: Bash\n---\n\nnew body"
    );

    let error = service
        .update_command_file(&CommandUpdateParams {
            command_id: None,
            config: config("/b", "clobber"),
            agent_source: None,
            old_file_path: Some(p(&home, ".mode/commands/a.md").to_string_lossy().into_owned()),
            storage_level: None,
            workspace_path: None,
        })
        .unwrap_err();
    assert_eq!(error.message, "Command file already exists: b.md");
}

#[test]
fn project_write_requires_workspace_path() {
    let root = TempRoot::new("project-missing");
    let home = root.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let service = service(&home);

    let error = service
        .write_command_file(&CommandCreateParams {
            config: config("/p", "body"),
            agent_source: None,
            storage_level: Some(CommandStorageLevel::Project),
            workspace_path: None,
        })
        .unwrap_err();
    assert_eq!(error.message, "Missing workspace path for project command");
}

#[test]
fn write_to_project_scope_uses_workspace_directory() {
    let root = TempRoot::new("project-write");
    let home = root.path().join("home");
    let workspace = root.path().join("ws");
    fs::create_dir_all(&workspace).unwrap();
    let service = service(&home);

    // 首次写不会替用户创建命名空间子目录（与 TS 一致），先建好。
    fs::create_dir_all(workspace.join(".mode").join("commands").join("deep")).unwrap();
    let written = service
        .write_command_file(&CommandCreateParams {
            config: config("/deep/cmd", "body"),
            agent_source: None,
            storage_level: Some(CommandStorageLevel::Project),
            workspace_path: Some(workspace.to_string_lossy().into_owned()),
        })
        .unwrap();

    assert_eq!(written.command.scope, commands_service::CommandScope::Project);
    assert_eq!(written.command.id, "modeAgent:mode:project:/deep/cmd");
    assert_eq!(
        written.command.project_path.as_deref(),
        Some(workspace.to_string_lossy().as_ref())
    );
    assert_eq!(
        fs::read_to_string(p(&workspace, ".mode/commands/deep/cmd.md")).unwrap(),
        "body"
    );
}

#[test]
fn write_rejects_path_escape() {
    let root = TempRoot::new("escape");
    let home = root.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let service = service(&home);

    for name in ["/../escape", "/a/../../escape"] {
        let error = service
            .write_command_file(&CommandCreateParams {
                config: config(name, "body"),
                agent_source: None,
                storage_level: None,
                workspace_path: None,
            })
            .unwrap_err();
        assert!(
            error.message.starts_with("Invalid command file name:"),
            "unexpected error for {name}: {}",
            error.message
        );
    }
    assert!(!home.join(".mode/escape.md").exists());
}

#[test]
fn nested_name_without_directory_fails_like_ts() {
    let root = TempRoot::new("nested-missing");
    let home = root.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let service = service(&home);

    let error = service
        .write_command_file(&CommandCreateParams {
            config: config("/nested/cmd", "body"),
            agent_source: None,
            storage_level: None,
            workspace_path: None,
        })
        .unwrap_err();
    assert!(
        error.message.starts_with("ENOENT: no such file or directory, open '"),
        "unexpected error: {}",
        error.message
    );
}

#[test]
fn primary_directory_is_created() {
    let root = TempRoot::new("primary");
    let home = root.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let service = service(&home);

    let result = service
        .get_primary_user_commands_directory(&PrimaryDirectoryParams::default())
        .unwrap();
    assert_eq!(
        result.path,
        p(&home, ".mode/commands").to_string_lossy()
    );
    assert!(p(&home, ".mode/commands").is_dir());
}

// ============================================================================
// 插件命令
// ============================================================================

fn write_plugin(
    root: &Path,
    name: &str,
    commands: Option<serde_json::Value>,
    files: &[(&str, &str)],
) {
    let manifest = match commands {
        Some(commands) => serde_json::json!({ "name": name, "commands": commands }),
        None => serde_json::json!({ "name": name }),
    };
    write(
        &root.join(".mode-plugin/plugin.json"),
        &serde_json::to_string(&manifest).unwrap(),
    );
    for (relative, content) in files {
        write(&root.join(relative), content);
    }
}

#[test]
fn plugin_commands_from_configured_dirs_are_enabled_by_default() {
    let root = TempRoot::new("plugin-dirs");
    let home = root.path().join("home");
    let plugin_root = root.path().join("plugin-a");
    write_plugin(
        &plugin_root,
        "plugin-a",
        Some(serde_json::json!("commands")),
        &[
            ("commands/alpha.md", "---\ndescription: alpha\n---\n\nalpha body"),
            ("commands/nested/beta.md", "beta body"),
            ("commands/notes.txt", "ignored"),
        ],
    );
    write_config(
        &home,
        serde_json::json!({
            "plugins": { "dirs": [plugin_root.to_string_lossy()] }
        }),
    );

    let service = service(&home);
    let result = list(&service, None);

    assert_eq!(
        result
            .plugin_commands
            .iter()
            .map(|command| command.name.clone())
            .collect::<Vec<_>>(),
        vec!["/alpha", "/nested/beta"]
    );
    let alpha = &result.plugin_commands[0];
    assert!(alpha.enabled);
    assert!(alpha.plugin_enabled);
    assert_eq!(alpha.plugin_marketplace, "inline");
    assert_eq!(alpha.plugin_name, "plugin-a");
    assert_eq!(alpha.description.as_deref(), Some("alpha"));
    assert_eq!(
        alpha.id,
        format!("plugin:inline:plugin-a:/alpha:{}", alpha.file_path)
    );
    // `commands` 字段合并：用户命令在前，插件命令在后。
    assert_eq!(result.commands.len(), 2);
}

#[test]
fn plugins_disabled_and_default_commands_directory() {
    let root = TempRoot::new("plugin-default-dir");
    let home = root.path().join("home");
    let plugin_root = root.path().join("plugin-b");
    write_plugin(&plugin_root, "plugin-b", None, &[("commands/x.md", "x body")]);

    write_config(
        &home,
        serde_json::json!({
            "plugins": { "dirs": [plugin_root.to_string_lossy()], "enabled": false }
        }),
    );
    let service = service(&home);
    assert!(list(&service, None).plugin_commands.is_empty());

    write_config(
        &home,
        serde_json::json!({
            "plugins": { "dirs": [plugin_root.to_string_lossy()] }
        }),
    );
    assert_eq!(list(&service, None).plugin_commands[0].name, "/x");
}

#[test]
fn official_cache_plugin_is_disabled_unless_enabled_in_config() {
    let root = TempRoot::new("plugin-cache");
    let home = root.path().join("home");
    // 用不在默认启用集合里的名字：pdf 等内置插件默认启用。
    let cache_root = p(&home, ".mode/cli/plugins/cache/mode-plugins-official/demo/1.0.0");
    write_plugin(
        &cache_root,
        "demo",
        Some(serde_json::json!("commands")),
        &[("commands/export.md", "demo body")],
    );

    let service = service(&home);
    assert!(list(&service, None).plugin_commands.is_empty());

    write_config(
        &home,
        serde_json::json!({
            "plugins": {
                "enabledPlugins": { "demo@mode-plugins-official": true }
            }
        }),
    );
    let result = list(&service, None);
    assert_eq!(result.plugin_commands.len(), 1);
    assert_eq!(result.plugin_commands[0].name, "/export");
    assert_eq!(
        result.plugin_commands[0].plugin_marketplace,
        "mode-plugins-official"
    );
}

#[test]
fn suppressed_builtins_skip_official_cache_plugins() {
    let root = TempRoot::new("plugin-suppressed");
    let home = root.path().join("home");
    let cache_root = p(&home, ".mode/cli/plugins/cache/mode-plugins-official/demo/1.0.0");
    write_plugin(
        &cache_root,
        "demo",
        Some(serde_json::json!("commands")),
        &[("commands/export.md", "demo body")],
    );
    write_config(
        &home,
        serde_json::json!({
            "plugins": {
                "enabledPlugins": { "demo@mode-plugins-official": true },
                "suppressedBuiltins": ["demo@zcode-plugins-official"]
            }
        }),
    );

    let service = service(&home);
    assert!(list(&service, None).plugin_commands.is_empty());
}

#[test]
fn installed_plugins_json_contributes_roots() {
    let root = TempRoot::new("plugin-installed");
    let home = root.path().join("home");
    let install_root = root.path().join("installed-plugin");
    write_plugin(
        &install_root,
        "installed",
        Some(serde_json::json!("commands")),
        &[("commands/run.md", "run body")],
    );
    write(
        &home.join(".mode/cli/plugins/installed_plugins.json"),
        &serde_json::to_string(&serde_json::json!({
            "plugins": [
                {
                    "id": "installed@my-market",
                    "marketplace": "my-market",
                    "installPath": install_root.to_string_lossy()
                },
                { "id": "relative", "marketplace": "m", "installPath": "relative/path" },
                { "id": "", "marketplace": "m", "installPath": install_root.to_string_lossy() }
            ]
        }))
        .unwrap(),
    );
    // installed_plugins.json 的候选默认关闭，必须显式启用。
    write_config(
        &home,
        serde_json::json!({ "plugins": { "enabledPlugins": { "installed@my-market": true } } }),
    );

    let service = service(&home);
    let result = list(&service, None);
    assert_eq!(result.plugin_commands.len(), 1);
    assert_eq!(result.plugin_commands[0].name, "/run");
    assert_eq!(result.plugin_commands[0].plugin_marketplace, "my-market");
}

#[test]
fn plugin_command_paths_outside_root_are_ignored() {
    let root = TempRoot::new("plugin-outside");
    let home = root.path().join("home");
    let plugin_root = root.path().join("plugin-c");
    write_plugin(
        &plugin_root,
        "plugin-c",
        Some(serde_json::json!(["../outside", "commands"])),
        &[("commands/ok.md", "ok body")],
    );
    write(&root.path().join("outside/leak.md"), "leak body");
    write_config(
        &home,
        serde_json::json!({ "plugins": { "dirs": [plugin_root.to_string_lossy()] } }),
    );

    let service = service(&home);
    let result = list(&service, None);
    assert_eq!(result.plugin_commands.len(), 1);
    assert_eq!(result.plugin_commands[0].name, "/ok");
}

#[test]
fn plugin_override_and_plugin_enabled_are_anded() {
    let root = TempRoot::new("plugin-override");
    let home = root.path().join("home");
    let plugin_root = root.path().join("plugin-d");
    write_plugin(&plugin_root, "plugin-d", None, &[("commands/one.md", "one")]);
    let file = p(&plugin_root, "commands/one.md");
    write_config(
        &home,
        serde_json::json!({
            "plugins": { "dirs": [plugin_root.to_string_lossy()] },
            "command": { (file.to_string_lossy()): { "enable": false } }
        }),
    );

    let service = service(&home);
    let result = list(&service, None);
    assert_eq!(result.plugin_commands.len(), 1);
    assert!(result.plugin_commands[0].plugin_enabled);
    assert!(!result.plugin_commands[0].enabled);
}

#[test]
fn invalid_manifest_is_skipped() {
    let root = TempRoot::new("plugin-invalid");
    let home = root.path().join("home");
    let plugin_root = root.path().join("plugin-e");
    write(
        &plugin_root.join(".mode-plugin/plugin.json"),
        "{\"name\":\"Bad Name\"}",
    );
    write(&plugin_root.join("commands/x.md"), "x");
    write_config(
        &home,
        serde_json::json!({ "plugins": { "dirs": [plugin_root.to_string_lossy()] } }),
    );

    let service = service(&home);
    assert!(list(&service, None).plugin_commands.is_empty());
}

#[test]
fn agent_source_filter_keeps_plugin_commands_for_mode_agent() {
    let root = TempRoot::new("agent-filter");
    let home = root.path().join("home");
    let plugin_root = root.path().join("plugin-f");
    write_plugin(&plugin_root, "plugin-f", None, &[("commands/z.md", "z")]);
    write(&home.join(".mode/commands/u.md"), "u");
    write_config(
        &home,
        serde_json::json!({ "plugins": { "dirs": [plugin_root.to_string_lossy()] } }),
    );

    let service = service(&home);
    let result = service
        .list(&ListParams {
            agent_source: Some(CommandAgentSource::ModeAgent),
            workspace_path: None,
            workspace_identity: Some("ignored".into()),
        })
        .unwrap();
    assert_eq!(names(&result.user_commands), vec!["/u"]);
    assert_eq!(result.plugin_commands.len(), 1);
}
