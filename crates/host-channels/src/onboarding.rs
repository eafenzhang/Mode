//! `onboarding-record` 频道适配器。
//!
//! 服务层在 `onboarding-record-service`（9/9 方法 TS 对拍）；本适配器只做位置参数
//! 解析与 `spawn_blocking`（文件 IO + 跨进程锁），不改变语义。
//! 规范见 `docs/specs/onboarding-record-service.md` §2.1。

use std::sync::Arc;

use onboarding_record_service::{
    OnboardingPreferencesPatch, OnboardingRecordEntryInput, OnboardingRecordService,
};
use rpc_host::{BoxFuture, CancellationToken, ChannelError, ServerChannel};
use rpc_wire::Value;
use serde::de::DeserializeOwned;

pub const ONBOARDING_RECORD_CHANNEL: &str = "onboarding-record";

pub struct OnboardingRecordChannel {
    service: Arc<OnboardingRecordService>,
}

impl OnboardingRecordChannel {
    pub fn new(service: Arc<OnboardingRecordService>) -> Self {
        Self { service }
    }
}

impl ServerChannel for OnboardingRecordChannel {
    fn call(
        &self,
        _ctx: String,
        command: String,
        arg: Value,
        _cancel: CancellationToken,
    ) -> BoxFuture<Result<Value, ChannelError>> {
        let service = Arc::clone(&self.service);
        Box::pin(async move {
            match command.as_str() {
                "appendRecord" => {
                    let device_mid = positional_string(&arg, 0, "deviceMid")?;
                    let entry: OnboardingRecordEntryInput = positional_json(&arg, 1, "entry")?;
                    run_blocking(move || service.append_record(&device_mid, &entry))
                        .await?
                        .map_err(ChannelError::error)?;
                    Ok(Value::Undefined)
                }
                "shouldOnboard" => {
                    let device_mid = positional_string(&arg, 0, "deviceMid")?;
                    let value = run_blocking(move || service.should_onboard(&device_mid)).await?;
                    json_value(&value.map_err(ChannelError::error)?)
                }
                "dismissOnboarding" => {
                    let device_mid = positional_string(&arg, 0, "deviceMid")?;
                    run_blocking(move || service.dismiss_onboarding(&device_mid))
                        .await?
                        .map_err(ChannelError::error)?;
                    Ok(Value::Undefined)
                }
                "claimAnonymousRecord" => {
                    run_blocking(move || service.claim_anonymous_record())
                        .await?
                        .map_err(ChannelError::error)?;
                    Ok(Value::Undefined)
                }
                "getLatestEntry" => {
                    let value = run_blocking(move || service.get_latest_entry()).await?;
                    json_value(&value.map_err(ChannelError::error)?)
                }
                "syncSettingsFromRecord" => {
                    let value = run_blocking(move || service.sync_settings_from_record()).await?;
                    json_value(&value.map_err(ChannelError::error)?)
                }
                "updateRecordPreferences" => {
                    let patch: OnboardingPreferencesPatch = positional_json(&arg, 0, "patch")?;
                    run_blocking(move || service.update_record_preferences(&patch))
                        .await?
                        .map_err(ChannelError::error)?;
                    Ok(Value::Undefined)
                }
                "getRecords" => {
                    let value = run_blocking(move || service.get_records()).await?;
                    json_value(&value.map_err(ChannelError::error)?)
                }
                "clearRecords" => {
                    run_blocking(move || service.clear_records())
                        .await?
                        .map_err(ChannelError::error)?;
                    Ok(Value::Undefined)
                }
                other => Err(ChannelError::error(format!(
                    "onboarding-record channel: unknown command {other}"
                ))),
            }
        })
    }
}

async fn run_blocking<T, F>(task: F) -> Result<T, ChannelError>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tokio::task::spawn_blocking(task)
        .await
        .map_err(|error| ChannelError::error(format!("onboarding task failed: {error}")))
}

fn json_value<T: serde::Serialize>(value: &T) -> Result<Value, ChannelError> {
    serde_json::to_string(value)
        .map(Value::Json)
        .map_err(|error| ChannelError::error(error.to_string()))
}

/// 位置参数：`ProxyChannel.toService` 把方法入参原样包成数组；单参数可能被压成裸值。
fn arg_at(arg: &Value, index: usize) -> Option<Value> {
    match arg {
        Value::Array(items) => items.get(index).cloned(),
        other if index == 0 => Some(other.clone()),
        _ => None,
    }
}

fn positional_string(arg: &Value, index: usize, name: &str) -> Result<String, ChannelError> {
    let Some(value) = arg_at(arg, index) else {
        return Err(ChannelError::error(format!("{name} is required")));
    };
    match value.to_json() {
        serde_json::Value::String(text) => Ok(text),
        _ => Err(ChannelError::error(format!("{name} must be a string"))),
    }
}

fn positional_json<T: DeserializeOwned>(
    arg: &Value,
    index: usize,
    name: &str,
) -> Result<T, ChannelError> {
    let Some(value) = arg_at(arg, index) else {
        return Err(ChannelError::error(format!("{name} is required")));
    };
    serde_json::from_value(value.to_json())
        .map_err(|error| ChannelError::error(format!("{name} invalid: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;
    use std::sync::Mutex;

    use onboarding_record_service::OnboardingRecordEnvironment;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(label: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "onboarding-channel-{label}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_nanos() as u64)
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&dir).expect("dir");
            TempDir(dir)
        }

        fn record_path(&self) -> PathBuf {
            self.0.join("onboarding-record.json")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct Env {
        user_id: Mutex<Option<String>>,
        has_task: bool,
    }

    impl OnboardingRecordEnvironment for Env {
        fn load_user_id(&self) -> Result<Option<String>, String> {
            Ok(self.user_id.lock().expect("user").clone())
        }

        fn has_existing_local_task(&self) -> Result<bool, String> {
            Ok(self.has_task)
        }
    }

    const DEVICE_MID: &str = "device-1";

    fn channel(dir: &TempDir, has_task: bool, user_id: Option<&str>) -> OnboardingRecordChannel {
        let service = OnboardingRecordService::new(
            dir.record_path(),
            Arc::new(Env {
                user_id: Mutex::new(user_id.map(str::to_string)),
                has_task,
            }),
            Arc::new(|| 1_700_000_000_000),
        );
        OnboardingRecordChannel::new(Arc::new(service))
    }

    async fn call(channel: &OnboardingRecordChannel, method: &str, args: Value) -> Value {
        channel
            .call(
                "ctx".into(),
                method.into(),
                args,
                CancellationToken::default(),
            )
            .await
            .expect("call")
    }

    fn entry() -> Value {
        Value::Json(
            serde_json::json!({
                "occupation": "developer",
                "interfaceMode": "coding",
                "memoryEnabled": true,
                "proactiveSuggestionsEnabled": true,
                "completedAt": "2026-11-22T00:00:00.000Z",
            })
            .to_string(),
        )
    }

    #[tokio::test]
    async fn append_then_read_latest_and_records() {
        let dir = TempDir::new("append");
        let channel = channel(&dir, false, None);
        call(
            &channel,
            "appendRecord",
            Value::Array(vec![Value::Str(DEVICE_MID.to_string()), entry()]),
        )
        .await;

        let latest = call(&channel, "getLatestEntry", Value::Array(vec![])).await;
        let Value::Json(text) = latest else {
            panic!("expected json");
        };
        let value: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert_eq!(value["occupation"], "developer");
        assert_eq!(value["uploadState"], "pending");
        assert_eq!(value["userId"], serde_json::Value::Null);

        let records = call(&channel, "getRecords", Value::Array(vec![])).await;
        let Value::Json(text) = records else {
            panic!("expected json");
        };
        let value: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert_eq!(value["deviceMid"], DEVICE_MID);
        assert_eq!(value["entries"].as_array().expect("entries").len(), 1);
    }

    #[tokio::test]
    async fn should_onboard_records_existing_local_user_decision() {
        let dir = TempDir::new("existing");
        let channel = channel(&dir, true, None);
        let answer = call(
            &channel,
            "shouldOnboard",
            Value::Array(vec![Value::Str(DEVICE_MID.to_string())]),
        )
        .await;
        let Value::Json(text) = answer else {
            panic!("expected json");
        };
        assert_eq!(text, "false");

        let records = call(&channel, "getRecords", Value::Array(vec![])).await;
        let Value::Json(text) = records else {
            panic!("expected json");
        };
        let value: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert_eq!(value["decisions"][0]["status"], "existing_local_user");
        assert_eq!(value["decisions"][0]["reason"], "existing_local_task");
    }

    #[tokio::test]
    async fn sync_settings_and_update_preferences_roundtrip() {
        let dir = TempDir::new("sync");
        let channel = channel(&dir, false, Some("user-1"));
        call(
            &channel,
            "appendRecord",
            Value::Array(vec![Value::Str(DEVICE_MID.to_string()), entry()]),
        )
        .await;

        let patch = call(&channel, "syncSettingsFromRecord", Value::Array(vec![])).await;
        let Value::Json(text) = patch else {
            panic!("expected json");
        };
        let value: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert_eq!(value["onboardingOccupation"], "developer");
        assert_eq!(value["memoryEnabled"], true);

        call(
            &channel,
            "updateRecordPreferences",
            Value::Array(vec![Value::Json(
                serde_json::json!({ "memoryEnabled": false }).to_string(),
            )]),
        )
        .await;
        let patch = call(&channel, "syncSettingsFromRecord", Value::Array(vec![])).await;
        let Value::Json(text) = patch else {
            panic!("expected json");
        };
        let value: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert_eq!(value["memoryEnabled"], false);
        assert_eq!(value["proactiveSuggestionsEnabled"], true);
    }

    #[tokio::test]
    async fn claim_anonymous_record_rewrites_entry() {
        let dir = TempDir::new("claim");
        let anonymous = channel(&dir, false, None);
        call(
            &anonymous,
            "appendRecord",
            Value::Array(vec![Value::Str(DEVICE_MID.to_string()), entry()]),
        )
        .await;

        let signed_in = channel(&dir, false, Some("user-1"));
        call(&signed_in, "claimAnonymousRecord", Value::Array(vec![])).await;
        let latest = call(&signed_in, "getLatestEntry", Value::Array(vec![])).await;
        let Value::Json(text) = latest else {
            panic!("expected json");
        };
        let value: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert_eq!(value["userId"], "user-1");
    }

    #[tokio::test]
    async fn clear_records_and_unknown_command() {
        let dir = TempDir::new("clear");
        let channel = channel(&dir, false, None);
        call(
            &channel,
            "appendRecord",
            Value::Array(vec![Value::Str(DEVICE_MID.to_string()), entry()]),
        )
        .await;
        call(&channel, "clearRecords", Value::Array(vec![])).await;
        let records = call(&channel, "getRecords", Value::Array(vec![])).await;
        let Value::Json(text) = records else {
            panic!("expected json");
        };
        assert_eq!(text, "null");

        let error = channel
            .call(
                "ctx".into(),
                "notARealMethod".into(),
                Value::Array(vec![]),
                CancellationToken::default(),
            )
            .await
            .expect_err("unknown command");
        match error {
            ChannelError::Error { message, .. } => {
                assert!(message.contains("unknown command"), "{message}");
            }
            other => panic!("期望 Error 变体，实际 {other:?}"),
        }
    }
}
