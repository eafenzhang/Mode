//! `plugin-sync` 频道：参数形状、嵌套 archive 编解码与导入导出。

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use host_channels::plugin_sync::{
    CHECK_REMOTE_USER_PLUGIN_WRITE_ACCESS, EXPORT_PLUGINS_ARCHIVE, IMPORT_PLUGINS_ARCHIVE,
    LIST_LOCAL_USER_PLUGIN_CANDIDATES, LIST_REMOTE_USER_PLUGIN_STATUSES,
};
use host_channels::PluginSyncChannel;
use rpc_host::{CancellationToken, ChannelError, ServerChannel};
use rpc_wire::Value;
use serde_json::{json, Value as Json};

struct TempHome(PathBuf);

impl TempHome {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "mode-plugin-sync-channel-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn add_plugin(&self, directory: &str, name: &str) -> PathBuf {
        let root = self.0.join("src").join(directory);
        fs::create_dir_all(root.join(".mode-plugin")).unwrap();
        fs::write(
            root.join(".mode-plugin").join("plugin.json"),
            format!("{{\"name\":\"{name}\"}}"),
        )
        .unwrap();
        fs::create_dir_all(root.join("skills").join("demo")).unwrap();
        fs::write(root.join("skills").join("demo").join("SKILL.md"), "# demo\n").unwrap();
        self.write_config(&[root.to_string_lossy().into_owned()], name);
        root
    }

    fn write_config(&self, dirs: &[String], name: &str) {
        let config_path = self.0.join(".mode").join("cli").join("config.json");
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        let config = json!({
            "plugins": {
                "dirs": dirs,
                "enabledPlugins": { format!("{name}@inline").to_lowercase(): false }
            }
        });
        fs::write(config_path, format!("{config}\n")).unwrap();
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn channel(home: &Path) -> PluginSyncChannel {
    PluginSyncChannel::new(host_channels::plugin_sync::service_for_home(
        home.to_path_buf(),
    ))
}

async fn call(
    channel: &PluginSyncChannel,
    method: &str,
    arg: Value,
) -> Result<Value, ChannelError> {
    channel
        .call(
            "ctx".into(),
            method.into(),
            arg,
            CancellationToken::default(),
        )
        .await
}

fn object(value: Json) -> Value {
    Value::Array(vec![Value::Json(value.to_string())])
}

fn json(value: &Value) -> Json {
    match value {
        Value::Json(text) => serde_json::from_str(text).expect("JSON body"),
        other => panic!("期望 JSON body，实际 {other:?}"),
    }
}

#[tokio::test]
async fn candidates_export_and_import_round_trip() {
    let source_home = TempHome::new("source");
    source_home.add_plugin("plugin-a", "Plugin A");
    let source_channel = channel(&source_home.0);

    let candidates = json(
        &call(&source_channel, LIST_LOCAL_USER_PLUGIN_CANDIDATES, object(json!({})))
            .await
            .unwrap(),
    );
    assert_eq!(candidates["candidates"][0]["name"], "Plugin A");
    assert_eq!(candidates["candidates"][0]["enabled"], false);
    assert_eq!(candidates["maxArchiveBytes"], 52428800);
    let candidate_id = candidates["candidates"][0]["id"].as_str().unwrap();

    let export = json(
        &call(
            &source_channel,
            EXPORT_PLUGINS_ARCHIVE,
            object(json!({ "pluginIds": [candidate_id] })),
        )
        .await
        .unwrap(),
    );
    assert!(export["archive"]["__mode_rpc_nested_uint8array_v1"]
        .as_bool()
        .unwrap());
    assert!(export["archiveBytes"].as_u64().unwrap() > 0);
    assert_eq!(export["plugins"][0]["directoryName"], "plugin-a");

    let target_home = TempHome::new("target");
    let target_channel = channel(&target_home.0);
    let imported = json(
        &call(
            &target_channel,
            IMPORT_PLUGINS_ARCHIVE,
            object(json!({ "archive": export["archive"] })),
        )
        .await
        .unwrap(),
    );
    assert_eq!(imported["results"][0]["status"], "synced");
    assert!(target_home
        .0
        .join(".mode")
        .join("plugins")
        .join("plugin-a")
        .join("skills")
        .join("demo")
        .join("SKILL.md")
        .exists());

    let again = json(
        &call(
            &target_channel,
            IMPORT_PLUGINS_ARCHIVE,
            object(json!({ "archive": export["archive"] })),
        )
        .await
        .unwrap(),
    );
    assert_eq!(again["results"][0]["status"], "skipped");

    let error = call(
        &target_channel,
        IMPORT_PLUGINS_ARCHIVE,
        object(json!({ "archive": export["archive"], "overwrite": true })),
    )
    .await
    .unwrap_err();
    match error {
        ChannelError::Error { message, .. } => {
            assert_eq!(message, "plugin sync overwrite is not supported")
        }
        other => panic!("期望 Error，实际 {other:?}"),
    }
}

#[tokio::test]
async fn statuses_and_write_access_and_unknown_command() {
    let home = TempHome::new("status");
    home.add_plugin("plugin-a", "Plugin A");
    let channel = channel(&home.0);

    fs::create_dir_all(home.0.join(".mode").join("plugins").join("existing")).unwrap();
    let statuses = json(
        &call(
            &channel,
            LIST_REMOTE_USER_PLUGIN_STATUSES,
            object(json!({
                "plugins": [
                    { "pluginId": "custom@inline", "directoryName": "existing" },
                    { "pluginId": "plugin a@inline", "directoryName": "new" },
                    { "pluginId": "none@inline", "directoryName": "none" }
                ]
            })),
        )
        .await
        .unwrap(),
    );
    assert_eq!(statuses["statuses"][0]["reason"], "targetExists");
    assert_eq!(statuses["statuses"][1]["reason"], "samePluginId");
    assert_eq!(statuses["statuses"][2]["exists"], false);

    let access = json(
        &call(
            &channel,
            CHECK_REMOTE_USER_PLUGIN_WRITE_ACCESS,
            object(json!({})),
        )
        .await
        .unwrap(),
    );
    assert_eq!(access["ok"], true);

    let error = call(&channel, "unknown", object(json!({}))).await.unwrap_err();
    match error {
        ChannelError::Error { message, .. } => {
            assert_eq!(message, "plugin-sync channel: unknown command unknown")
        }
        other => panic!("期望 Error，实际 {other:?}"),
    }
}
