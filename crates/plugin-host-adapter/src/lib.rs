//! `SidecarSupervisor` → `PluginToolSource` 的适配层（M3 生态域 1 · Task 4 Step 1）。
//!
//! 这一层只做协议映射：`plugin/listTools` → [`PluginToolDeclaration`]，
//! `plugin/callTool` → sidecar 返回的 `{content}`。权限仍在会话层（`permission::evaluate`），
//! 本适配器不判断风险。

use std::sync::Arc;

use agent_core::plugin_tools::{PluginToolDeclaration, PluginToolSource};
use plugin_host::protocol::Method;
use plugin_host::supervisor::{SidecarSupervisor, SupervisorError};
use serde_json::{json, Value};

pub struct PluginHostSource {
    supervisor: Arc<SidecarSupervisor>,
}

impl PluginHostSource {
    pub fn new(supervisor: Arc<SidecarSupervisor>) -> Self {
        Self { supervisor }
    }

    fn request(&self, method: Method, params: Value) -> Result<Value, String> {
        self.supervisor
            .request(method, params)
            .map_err(|error: SupervisorError| error.to_string())
    }
}

impl PluginToolSource for PluginHostSource {
    fn list_tools(&self) -> Result<Vec<PluginToolDeclaration>, String> {
        let result = self.request(Method::ListTools, json!({}))?;
        let tools = result
            .get("tools")
            .and_then(Value::as_array)
            .ok_or_else(|| "plugin/listTools 的结果缺少 tools 数组".to_string())?;
        tools
            .iter()
            .map(|tool| {
                let name = tool
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "tool 缺少 name".to_string())?;
                let description = tool
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let input_schema = tool
                    .get("inputSchema")
                    .cloned()
                    .ok_or_else(|| format!("tool {name} 缺少 inputSchema"))?;
                // 三个可选元数据；sidecar 不声明时由 `into_spec` 取插件 MCP 的默认值。
                let optional =
                    |key: &str| tool.get(key).and_then(Value::as_str).map(str::to_string);
                Ok(PluginToolDeclaration {
                    name: name.to_string(),
                    description: description.to_string(),
                    input_schema,
                    risk_level: optional("riskLevel"),
                    side_effect_scope: optional("sideEffectScope"),
                    budget_strategy: optional("budgetStrategy"),
                })
            })
            .collect()
    }

    fn call_tool(&self, tool_name: &str, input: &Value) -> Result<Value, String> {
        self.request(
            Method::CallTool,
            json!({ "toolName": tool_name, "input": input }),
        )
    }
}
