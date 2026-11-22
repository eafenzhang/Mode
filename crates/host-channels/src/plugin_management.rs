//! `plugin-management` 频道（薄别名）：设置页插件管理 → `mode-agent` 同名转发。
//!
//! 规范见 `docs/specs/plugin-management-channel.md`。唯一加工是
//! `getPluginsOverview` 补 `officialMarketplaceEnabled: true`（TS 薄服务同口径）。

use std::sync::Arc;

use rpc_host::{
    BoxFuture, CancellationToken, ChannelError, EventCallback, ServerChannel, Subscription,
};
use rpc_wire::Value;

pub const PLUGIN_MANAGEMENT_CHANNEL: &str = "plugin-management";

pub struct PluginManagementChannel {
    agent: Arc<dyn ServerChannel>,
}

impl PluginManagementChannel {
    pub fn new(agent: Arc<dyn ServerChannel>) -> Self {
        Self { agent }
    }
}

impl ServerChannel for PluginManagementChannel {
    fn call(
        &self,
        ctx: String,
        command: String,
        arg: Value,
        cancel: CancellationToken,
    ) -> BoxFuture<Result<Value, ChannelError>> {
        let agent = Arc::clone(&self.agent);
        Box::pin(async move {
            let value = agent.call(ctx, command.clone(), arg, cancel).await?;
            if command == "getPluginsOverview" {
                return Ok(with_official_marketplace(value));
            }
            Ok(value)
        })
    }

    fn listen(
        &self,
        ctx: String,
        event: String,
        arg: Value,
        emit: EventCallback,
    ) -> Result<Subscription, ChannelError> {
        self.agent.listen(ctx, event, arg, emit)
    }
}

/// TS 薄服务：公开市场目录随包内置，恒置 `officialMarketplaceEnabled: true`。
fn with_official_marketplace(value: Value) -> Value {
    let Value::Json(text) = value else {
        return value;
    };
    let Ok(mut json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Value::Json(text);
    };
    let Some(object) = json.as_object_mut() else {
        return Value::Json(text);
    };
    if object.get("officialMarketplaceEnabled") == Some(&serde_json::Value::Bool(true)) {
        return Value::Json(text);
    }
    object.insert(
        "officialMarketplaceEnabled".to_string(),
        serde_json::Value::Bool(true),
    );
    match serde_json::to_string(&json) {
        Ok(text) => Value::Json(text),
        Err(_) => Value::Json(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;

    struct FakeAgent {
        calls: Mutex<Vec<String>>,
        overview: Value,
    }

    impl ServerChannel for FakeAgent {
        fn call(
            &self,
            _ctx: String,
            command: String,
            _arg: Value,
            _cancel: CancellationToken,
        ) -> BoxFuture<Result<Value, ChannelError>> {
            self.calls.lock().expect("calls").push(command.clone());
            let value = if command == "getPluginsOverview" {
                self.overview.clone()
            } else {
                Value::Json("null".to_string())
            };
            Box::pin(async move { Ok(value) })
        }

        fn listen(
            &self,
            _ctx: String,
            event: String,
            _arg: Value,
            _emit: EventCallback,
        ) -> Result<Subscription, ChannelError> {
            if event == "onDynamicPluginOperationProgress" {
                Ok(Box::new(|| {}))
            } else {
                Err(ChannelError::error("unsupported"))
            }
        }
    }

    fn channel(overview: Value) -> (PluginManagementChannel, Arc<FakeAgent>) {
        let agent = Arc::new(FakeAgent {
            calls: Mutex::new(Vec::new()),
            overview,
        });
        (
            PluginManagementChannel::new(Arc::clone(&agent) as Arc<dyn ServerChannel>),
            agent,
        )
    }

    #[tokio::test]
    async fn forwards_calls_and_projects_overview() {
        let (channel, agent) = channel(Value::Json(
            serde_json::json!({ "marketplaces": [], "plugins": [] }).to_string(),
        ));
        let result = channel
            .call(
                "ctx".into(),
                "getPluginsOverview".into(),
                Value::Json("{}".to_string()),
                CancellationToken::default(),
            )
            .await
            .expect("call");
        let Value::Json(text) = result else {
            panic!("expected json");
        };
        let value: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert_eq!(value["officialMarketplaceEnabled"], true);
        assert_eq!(
            agent.calls.lock().expect("calls").as_slice(),
            &["getPluginsOverview".to_string()]
        );
    }

    #[tokio::test]
    async fn forwards_names_for_other_methods() {
        let (channel, agent) = channel(Value::Json("{}".to_string()));
        channel
            .call(
                "ctx".into(),
                "installPlugin".into(),
                Value::Json("{}".to_string()),
                CancellationToken::default(),
            )
            .await
            .expect("call");
        assert_eq!(
            agent.calls.lock().expect("calls").as_slice(),
            &["installPlugin".to_string()]
        );
    }

    #[test]
    fn listens_delegate_to_agent() {
        let (channel, _agent) = channel(Value::Json("{}".to_string()));
        let emit: EventCallback = Arc::new(|_| {});
        let subscription = channel
            .listen(
                "ctx".into(),
                "onDynamicPluginOperationProgress".into(),
                Value::Json("{}".to_string()),
                emit,
            )
            .expect("listen");
        drop(subscription);
        assert!(channel
            .listen(
                "ctx".into(),
                "unknownEvent".into(),
                Value::Json("{}".to_string()),
                Arc::new(|_| {}),
            )
            .is_err());
    }
}
