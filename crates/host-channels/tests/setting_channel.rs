//! `setting` 频道适配器：读默认值、补丁更新、默认项目、数据目录指针。

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use host_channels::setting::{ENSURE_DEFAULT_PROJECT, GET, UPDATE, UPDATE_DATA_BASE_DIR};
use host_channels::SettingChannel;
use rpc_host::{CancellationToken, ServerChannel};
use rpc_wire::Value;
use settings_store::resolve_settings_paths;
use settings_store::{HomeEnv, SettingsService};

fn unique_home(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let home = std::env::temp_dir().join(format!(
        "mode-setting-channel-{name}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&home);
    fs::create_dir_all(&home).expect("建临时 home");
    home
}

fn channel(home: &PathBuf) -> SettingChannel {
    let env = HomeEnv {
        mode_desktop_home_dir: None,
        home: Some(home.clone().into_os_string()),
        user_profile: None,
        fallback_home: home.clone(),
    };
    let paths = resolve_settings_paths(&env);
    SettingChannel::new(
        Arc::new(SettingsService::new(paths, home.clone())),
        home.clone(),
    )
}

fn body(value: &Value) -> serde_json::Value {
    match value {
        Value::Json(text) => serde_json::from_str(text).expect("JSON body"),
        other => panic!("期望 JSON body，实际 {other:?}"),
    }
}

#[tokio::test]
async fn get_returns_defaults_and_update_persists_patch() {
    let home = unique_home("get");
    let channel = channel(&home);

    let initial = body(
        &channel
            .call("ctx".into(), GET.into(), Value::Array(Vec::new()), CancellationToken::default())
            .await
            .expect("读取"),
    );
    assert_eq!(initial["locale"], "zh-CN");

    let patch = Value::Array(vec![Value::Json(r#"{"locale":"en-US"}"#.into())]);
    channel
        .call("ctx".into(), UPDATE.into(), patch, CancellationToken::default())
        .await
        .expect("更新");

    let updated = body(
        &channel
            .call("ctx".into(), GET.into(), Value::Array(Vec::new()), CancellationToken::default())
            .await
            .expect("再读"),
    );
    assert_eq!(updated["locale"], "en-US");
    assert!(home.join(".mode/v2/setting.json").is_file());
}

#[tokio::test]
async fn ensure_default_project_reports_creation_once() {
    let home = unique_home("project");
    let channel = channel(&home);
    let arg = Value::Array(vec![Value::Str(home.to_string_lossy().into_owned())]);

    let first = body(
        &channel
            .call("ctx".into(), ENSURE_DEFAULT_PROJECT.into(), arg.clone(), CancellationToken::default())
            .await
            .expect("首次"),
    );
    assert_eq!(first["created"], true);
    let second = body(
        &channel
            .call("ctx".into(), ENSURE_DEFAULT_PROJECT.into(), arg, CancellationToken::default())
            .await
            .expect("再次"),
    );
    assert_eq!(second["created"], false);
}

#[tokio::test]
async fn update_data_base_dir_writes_pointer() {
    let home = unique_home("basedir");
    let channel = channel(&home);
    let new_base = unique_home("basedir-new");
    let arg = Value::Array(vec![Value::Str(new_base.to_string_lossy().into_owned())]);

    channel
        .call("ctx".into(), UPDATE_DATA_BASE_DIR.into(), arg, CancellationToken::default())
        .await
        .expect("更新数据目录");

    let document: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(home.join(".mode/v2/setting.json")).expect("读 setting"))
            .expect("JSON");
    assert_eq!(document["dataBaseDir"], new_base.to_string_lossy().as_ref());
}
