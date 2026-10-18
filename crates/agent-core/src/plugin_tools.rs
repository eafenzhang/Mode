//! 插件工具源（M3 生态域 1 · Task 3/5）。
//!
//! 会话只依赖这个 trait；真正的实现（Node sidecar / `plugin/listTools`）在
//! `plugin-host` 或 host 层。工具声明来自运行时，因此 `ToolSpec.name` 是 `String`。
//!
//! # 观察到的元数据（`probe-plugin-mcp.mts`，2026-10-18）
//!
//! | 工具 | riskLevel | sideEffectScope | budgetStrategy |
//! | --- | --- | --- | --- |
//! | 插件 MCP（`mcp__plugin_fixture_fixture__echo`） | `medium` | `network` | `truncate` |
//! | 内置 `mcp__node_repl__js` | `high` | `system` | `artifact` |
//!
//! 因此插件工具不能一律 high/artifact；声明里允许携带这三个字段，缺省取
//! **medium / network / truncate**（插件 MCP 的观察值）。内置 node_repl 的特例由宿主在
//! 声明里显式给出 high/system/artifact。

use serde_json::Value;

use agent_tools::{ModeToolFamily, PluginToolMetadata, ToolSpec};

#[derive(Debug, Clone, PartialEq)]
pub struct PluginToolDeclaration {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    /// 可选：sidecar 没给时按插件 MCP 的默认值。
    pub risk_level: Option<String>,
    pub side_effect_scope: Option<String>,
    pub budget_strategy: Option<String>,
}

impl PluginToolDeclaration {
    pub fn into_spec(self) -> ToolSpec {
        let defaults = PluginToolMetadata::default();
        ToolSpec {
            name: self.name,
            family: ModeToolFamily::TaskControl,
            schema: self.input_schema,
            read_only: false,
            plugin: Some(PluginToolMetadata {
                risk_level: self.risk_level.unwrap_or(defaults.risk_level),
                side_effect_scope: self.side_effect_scope.unwrap_or(defaults.side_effect_scope),
                budget_strategy: self.budget_strategy.unwrap_or(defaults.budget_strategy),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginToolError {
    pub message: String,
}

pub trait PluginToolSource: Send + Sync {
    fn list_tools(&self) -> Result<Vec<PluginToolDeclaration>, String>;
    /// 返回值约定为 `{ "content": "<工具正文>" }`（与 sidecar 的 `plugin/callTool` 一致）。
    fn call_tool(&self, tool_name: &str, input: &Value) -> Result<Value, String>;
}
