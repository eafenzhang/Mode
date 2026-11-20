//! C4 T2：Plugins 读/写/市场/安装的集成测试。
//!
//! 观察基线：`docs/specs/agent-c4-contract.md` §3 与 `fixtures/c4/plugins-*.json`。
//! 使用隔离 home/dataRoot；不读真实用户目录、不访问网络。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use agent_protocol::plugin::{
    PluginComponentKind, PluginSuggestedReferenceStatus, PluginUpdateStatus,
};
use agent_server::PluginService;
use serde_json::json;

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mode-agent-server-plugins-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp root");
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

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, text).expect("write file");
}

/// 与 `probe-c4-support.mts` 的 `writeLocalMarketplace` 同一布局（本地 directory marketplace）。
fn seed_marketplace(root: &Path) {
    write(
        &root.join("marketplace.json"),
        &serde_json::to_string_pretty(&json!({
            "name": "fixture-marketplace",
            "owner": { "name": "C4 Probe" },
            "plugins": [{
                "name": "fixture",
                "source": "./plugins/fixture",
                "description": "C4 probe fixture plugin fixture"
            }]
        }))
        .expect("json"),
    );
    let plugin = root.join("plugins").join("fixture");
    write(
        &plugin.join(".mode-plugin").join("plugin.json"),
        &serde_json::to_string_pretty(&json!({
            "name": "fixture",
            "version": "1.0.0",
            "description": "C4 probe fixture plugin fixture"
        }))
        .expect("json"),
    );
    write(
        &plugin.join("commands").join("hello.md"),
        "---\ndescription: Fixture command\n---\n\nHello from the C4 fixture plugin.\n",
    );
    write(
        &plugin
            .join("skills")
            .join("fixture-skill")
            .join("SKILL.md"),
        "---\nname: fixture-skill\ndescription: Use when the C4 probe asks to exercise the fixture plugin skill.\n---\n\nfixture skill body.\n",
    );
    write(
        &plugin.join(".mcp.json"),
        &serde_json::to_string_pretty(&json!({
            "mcpServers": { "fixture": { "command": "node", "args": ["mcp-server.mjs"] } }
        }))
        .expect("json"),
    );
    write(&plugin.join("mcp-server.mjs"), "// fixture\n");
}

fn service(root: &TempRoot) -> PluginService {
    let home = root.path().join("home");
    let data_root = home.join(".mode");
    fs::create_dir_all(&data_root).expect("data root");
    PluginService::with_paths(home, data_root)
}

#[test]
fn marketplace_add_install_list_describe_disable_uninstall() {
    let root = TempRoot::new("lifecycle");
    let marketplace_root = root.path().join("marketplace");
    seed_marketplace(&marketplace_root);
    let service = service(&root);

    let added = service
        .marketplace_add(marketplace_root.to_str().expect("path"))
        .expect("add marketplace");
    let summary = added.marketplace.expect("marketplace summary");
    assert_eq!(summary.name, "fixture-marketplace");
    assert_eq!(summary.plugin_count, 1);
    assert_eq!(summary.source["source"], json!("directory"));

    // 安装前：插件不在根目录集合里（只有 marketplace 快照）。
    assert!(service.list().plugins.is_empty());
    let overview = service.overview();
    assert_eq!(overview.marketplaces.len(), 1);
    assert_eq!(overview.available_plugins.len(), 1);
    assert!(!overview.available_plugins[0].installed);
    assert!(overview.installed_plugins.is_empty());

    let installed = service
        .install("fixture", "fixture-marketplace", None)
        .expect("install");
    assert_eq!(
        installed.dependency_closure,
        vec!["fixture@fixture-marketplace"]
    );
    assert_eq!(installed.installed_plugins.len(), 1);
    let install_path = installed.installed_plugins[0]
        .install_path
        .clone()
        .expect("install path");
    assert!(Path::new(&install_path)
        .join(".mode-plugin/plugin.json")
        .is_file());

    let list = service.list();
    let plugin = list
        .plugins
        .iter()
        .find(|plugin| plugin.id == "fixture@fixture-marketplace")
        .expect("installed plugin listed");
    assert!(plugin.enabled);
    assert_eq!(plugin.source, "cache");
    assert_eq!(plugin.marketplace, "fixture-marketplace");
    assert_eq!(plugin.version.as_deref(), Some("1.0.0"));
    assert_eq!(plugin.skill_count, Some(1));
    assert_eq!(plugin.command_root_count, 1);
    assert_eq!(
        plugin.declared_mcp_server_names,
        Some(vec!["fixture".to_string()])
    );
    assert_eq!(
        plugin.mcp_server_names,
        vec!["plugin:fixture:fixture".to_string()]
    );
    let components = plugin.components.as_ref().expect("components");
    let kinds: Vec<_> = components.iter().map(|group| group.kind).collect();
    assert_eq!(
        kinds,
        vec![
            PluginComponentKind::Command,
            PluginComponentKind::Skill,
            PluginComponentKind::Mcp
        ]
    );

    let describe = service.describe("fixture-marketplace", "fixture");
    assert!(describe.diagnostics.is_none());
    assert_eq!(
        describe.metadata.expect("metadata").version.as_deref(),
        Some("1.0.0")
    );
    assert_eq!(describe.components.len(), 3);

    let overview = service.overview();
    assert_eq!(overview.installed_plugins.len(), 1);
    assert!(overview.available_plugins[0].installed);

    let mcp_servers = service.session_mcp_servers();
    assert_eq!(mcp_servers.len(), 1);
    assert_eq!(mcp_servers[0].plugin_name, "fixture");
    match &mcp_servers[0].server {
        agent_protocol::session_create::McpServer::Stdio(server) => {
            assert_eq!(server.name, "plugin:fixture:fixture");
            assert_eq!(server.command, "node");
            assert_eq!(server.args, vec!["mcp-server.mjs".to_string()]);
        }
        other => panic!("expected stdio plugin mcp server, got {other:?}"),
    }

    let disabled = service
        .set_enabled("fixture@fixture-marketplace", false)
        .expect("disable");
    assert!(!disabled.plugin.enabled);
    assert!(!service.list().plugins[0].enabled);
    let enabled = service
        .set_enabled("fixture@fixture-marketplace", true)
        .expect("enable");
    assert!(enabled.plugin.enabled);

    let uninstalled = service
        .uninstall(Some("fixture@fixture-marketplace"), None, None, None)
        .expect("uninstall");
    let removed = uninstalled.removed_plugin.expect("removed plugin");
    assert_eq!(removed.name, "fixture");
    assert!(!removed.enabled);
    assert!(service.list().plugins.is_empty());
    assert!(!Path::new(&install_path).exists());

    service
        .marketplace_remove("fixture-marketplace")
        .expect("remove marketplace");
    assert!(service.overview().marketplaces.is_empty());
}

#[test]
fn describe_unknown_marketplace_is_success_with_diagnostic() {
    let root = TempRoot::new("describe-missing");
    let service = service(&root);
    let describe = service.describe("nonexistent-marketplace", "browser-use");
    assert!(describe.components.is_empty());
    let diagnostics = describe.diagnostics.expect("diagnostics");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code, "plugin_marketplace_invalid");
    assert_eq!(
        diagnostics[0].message,
        "Marketplace not found: nonexistent-marketplace"
    );
}

#[test]
fn official_cache_plugins_are_listed_and_resolved() {
    let root = TempRoot::new("official");
    let service = service(&root);
    let plugin_root = root
        .path()
        .join("home/.mode/cli/plugins/cache/mode-plugins-official/browser-use/0.5.1");
    write(
        &plugin_root.join(".mode-plugin/plugin.json"),
        &serde_json::to_string_pretty(&json!({
            "name": "browser-use",
            "version": "0.5.1",
            "description": "Browser automation skills",
            "author": "Mode"
        }))
        .expect("json"),
    );
    write(
        &plugin_root.join("skills/control-browser/SKILL.md"),
        "---\nname: control-browser\ndescription: Control the browser.\n---\n\nbody\n",
    );

    let list = service.list();
    let plugin = list
        .plugins
        .iter()
        .find(|plugin| plugin.name == "browser-use")
        .expect("official plugin");
    assert_eq!(plugin.source, "official");
    assert_eq!(plugin.marketplace, "mode-plugins-official");
    assert!(plugin.enabled);
    assert_eq!(plugin.skill_count, Some(1));

    let catalog =
        service.reference_catalog(agent_protocol::plugin::PluginReferenceAuthority::Workspace);
    assert!(catalog
        .plugins
        .iter()
        .any(|entry| entry.plugin_id == "browser-use@mode-plugins-official"));

    let ready = service.resolve_suggested_reference("browser-use@mode-plugins-official");
    assert_eq!(ready.status, PluginSuggestedReferenceStatus::Ready);
    assert_eq!(ready.marketplace.as_deref(), Some("mode-plugins-official"));
    assert_eq!(ready.plugin_name.as_deref(), Some("browser-use"));
    assert_eq!(
        ready.source_trust,
        Some(agent_protocol::plugin::SourceTrustOfficial::Official)
    );

    let bare = service.resolve_suggested_reference("browser-use");
    assert_eq!(bare.status, PluginSuggestedReferenceStatus::Unavailable);
    assert_eq!(
        bare.diagnostics[0].code,
        "plugin_suggested_reference_untrusted_source"
    );

    let missing = service.resolve_suggested_reference("mimosa@mode-plugins-official");
    assert_eq!(missing.status, PluginSuggestedReferenceStatus::Unavailable);
    assert_eq!(missing.diagnostics[0].code, "marketplace_refresh_failed");
    assert_eq!(
        missing.diagnostics[0].message,
        "Marketplace not found: mode-plugins-official"
    );

    // 本地 marketplace 的引用不是官方可信来源。
    let marketplace_root = root.path().join("marketplace");
    seed_marketplace(&marketplace_root);
    service
        .marketplace_add(marketplace_root.to_str().expect("path"))
        .expect("add marketplace");
    let local = service.resolve_suggested_reference("fixture@fixture-marketplace");
    assert_eq!(local.status, PluginSuggestedReferenceStatus::Unavailable);
    assert_eq!(
        local.diagnostics[0].code,
        "plugin_suggested_reference_untrusted_source"
    );
}

#[test]
fn marketplace_update_refreshes_snapshot_and_plugin_count() {
    let root = TempRoot::new("update-market");
    let marketplace_root = root.path().join("marketplace");
    seed_marketplace(&marketplace_root);
    let service = service(&root);
    service
        .marketplace_add(marketplace_root.to_str().expect("path"))
        .expect("add marketplace");

    write(
        &marketplace_root.join("marketplace.json"),
        &serde_json::to_string_pretty(&json!({
            "name": "fixture-marketplace",
            "owner": { "name": "C4 Probe" },
            "plugins": [
                { "name": "fixture", "source": "./plugins/fixture", "description": "C4 probe fixture plugin fixture" },
                { "name": "second", "source": "./plugins/second", "description": "second" }
            ]
        }))
        .expect("json"),
    );
    let updated = service
        .marketplace_update(Some("fixture-marketplace"))
        .expect("update");
    let marketplaces = updated.marketplaces.expect("marketplaces");
    assert_eq!(marketplaces[0].plugin_count, 2);
    assert_eq!(service.overview().available_plugins.len(), 2);
}

#[test]
fn installed_summary_carries_installation_metadata() {
    let root = TempRoot::new("summary");
    let marketplace_root = root.path().join("marketplace");
    seed_marketplace(&marketplace_root);
    let service = service(&root);
    service
        .marketplace_add(marketplace_root.to_str().expect("path"))
        .expect("add marketplace");
    let installed = service
        .install("fixture", "fixture-marketplace", None)
        .expect("install");
    let summary = &installed.installed_plugins[0];
    assert_eq!(summary.id, "fixture@fixture-marketplace");
    assert_eq!(summary.name, "fixture");
    assert_eq!(summary.marketplace, "fixture-marketplace");
    assert_eq!(summary.version.as_deref(), Some("1.0.0"));
    assert!(summary.enabled);
    assert_eq!(summary.scope, agent_protocol::plugin::PluginScope::User);
    assert!(summary.installed_at.is_some());
    assert_eq!(summary.update_status, None::<PluginUpdateStatus>);
    assert!(summary
        .component_types
        .as_ref()
        .expect("component types")
        .contains(&"skill".to_string()));
}

#[test]
fn marketplace_and_install_reject_path_traversal() {
    let root = TempRoot::new("traversal");
    let marketplace_root = root.path().join("marketplace");
    seed_marketplace(&marketplace_root);

    // 1) marketplace 名不能把快照目录带出 plugins 根。
    let manifest = json!({
        "name": "../escaped-marketplace",
        "owner": { "name": "C4 Probe" },
        "plugins": []
    });
    write(
        &marketplace_root.join("marketplace.json"),
        &serde_json::to_string_pretty(&manifest).expect("json"),
    );
    let service = service(&root);
    let error = service
        .marketplace_add(marketplace_root.to_str().expect("path"))
        .expect_err("traversal marketplace name must fail");
    assert!(error.contains("Invalid marketplace name"), "{error}");

    // 2) marketplace.json 的 plugin source 不能逃出快照目录。
    seed_marketplace(&marketplace_root);
    service
        .marketplace_add(marketplace_root.to_str().expect("path"))
        .expect("add marketplace");
    let manifest = json!({
        "name": "fixture-marketplace",
        "owner": { "name": "C4 Probe" },
        "plugins": [{
            "name": "fixture",
            "source": "../../../../outside-plugin",
            "description": "escape attempt"
        }]
    });
    write(
        &marketplace_root.join("marketplace.json"),
        &serde_json::to_string_pretty(&manifest).expect("json"),
    );
    service
        .marketplace_update(Some("fixture-marketplace"))
        .expect("refresh snapshot");
    let error = service
        .install("fixture", "fixture-marketplace", None)
        .expect_err("escaping plugin source must fail");
    assert!(error.contains("escapes marketplace"), "{error}");

    // 3) remove 的市场 id 同样是路径段。
    let error = service
        .marketplace_remove("../../escaped-marketplace")
        .expect_err("traversal marketplace id must fail");
    assert!(error.contains("Invalid marketplace"), "{error}");
    assert!(!root.path().join("escaped-marketplace").exists());
}

#[test]
fn uninstall_never_deletes_outside_plugin_cache() {
    let root = TempRoot::new("uninstall-outside");
    let marketplace_root = root.path().join("marketplace");
    seed_marketplace(&marketplace_root);
    let service = service(&root);
    service
        .marketplace_add(marketplace_root.to_str().expect("path"))
        .expect("add marketplace");
    let installed = service
        .install("fixture", "fixture-marketplace", None)
        .expect("install");
    let install_path = installed.installed_plugins[0]
        .install_path
        .as_deref()
        .expect("install path");
    // <storage>/cache/<marketplace>/<plugin>/<version> → 向上 4 级是 plugins 存储根。
    let storage_root = Path::new(install_path)
        .ancestors()
        .nth(4)
        .expect("storage root")
        .to_path_buf();
    let installed_file = storage_root.join("installed_plugins.json");
    let mut document: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&installed_file).expect("read installed"))
            .expect("parse installed");
    let outside = root.path().join("outside-delete");
    write(&outside.join("keep.txt"), "keep\n");
    document["plugins"][0]["installPath"] = json!(outside.display().to_string());
    fs::write(
        &installed_file,
        serde_json::to_string_pretty(&document).expect("json"),
    )
    .expect("write installed");

    service
        .uninstall(Some("fixture@fixture-marketplace"), None, None, Some(true))
        .expect("uninstall with tampered path");
    assert!(
        outside.join("keep.txt").exists(),
        "cache 根外的路径不能被删除"
    );
}
