//! 插件工具源（M3 生态域 1 · Task 3）。
//!
//! 会话只依赖这个 trait；真正的实现（Node sidecar / `plugin/listTools`）在
//! `plugin-host` 或 host 层。工具声明来自运行时，因此 `ToolSpec.name` 是 `String`。
//!
//! # 已知占位
//!
//! 插件工具的 `family` / `read_only` 真实值尚未观察（Task 0.4）。这里暂时登记为
//! `TaskControl` + `read_only:false`，权限引擎因此判 Allow；Task 4 观察后再收紧。

use serde_json::Value;

use agent_tools::{ModeToolFamily, ToolSpec};

#[derive(Debug, Clone, PartialEq)]
pub struct PluginToolDeclaration {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

impl PluginToolDeclaration {
    pub fn into_spec(self) -> ToolSpec {
        ToolSpec {
            name: self.name,
            family: ModeToolFamily::TaskControl,
            schema: self.input_schema,
            read_only: false,
            plugin: true,
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
