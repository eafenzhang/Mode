//! 测试专用 provider 装配源：从 oracle 行为夹具的 `script` 构造
//! [`agent_core::provider::ScriptedProvider`]（LT-1 C1 Task 10 三层对拍）。
//!
//! **仅测试可达**：由 `MODE_AGENT_SERVER_SCRIPTED_PROVIDER=<夹具路径>` 显式开启，
//! 没有 env 就完全不参与装配（生产路径永远是 [`crate::dispatch::HubProviderSource`]）。
//! 环境变量存在但文件不可用时**不在启动时崩溃**——错误延迟到 `resolve`，首个 turn
//! 以 `CONFIGURATION_ERROR` 显式失败（与「会话没有可用模型」同一出口），避免一个
//! 拼错的 env 让整个 stdio 服务静默退化为不可用。
//!
//! 剧本语义与 TS mock provider 同一口径（`mock-provider.mts` 的确定性声明）：
//! 下标 = 请求里 `Assistant` 消息条数，越界重复最后一步（该规则在
//! `ScriptedProvider` 内部实现，这里只做形状映射与 `${cwd}` 替换）：
//! - `{"text": "…"}`                → `ScriptStep::Text`；
//! - `{"toolCalls": [{"name","input"}]}` → `ScriptStep::ToolCalls`；
//! - 其他形状（函数步不存在于 JSON）→ 显式解析错误，不猜。

use std::path::PathBuf;
use std::sync::Arc;

use agent_core::provider::{ScriptStep, ScriptToolCall, ScriptedProvider};
use agent_core::ModelProvider;
use serde_json::Value;

use crate::runtime::ProviderSource;

/// 测试开关环境变量（`Task 10`；写入文档 `tauri-agent-runtime.md` §C1 差异登记）。
pub const SCRIPTED_PROVIDER_ENV: &str = "MODE_AGENT_SERVER_SCRIPTED_PROVIDER";

/// 夹具剧本里的工作目录占位符（与 TS `substituteCwd` 同词）。
const CWD_TOKEN: &str = "${cwd}";

/// 从夹具构造的装配源：构造时读盘解析，resolve 时替换占位符。
pub struct ScriptedFixtureSource {
    fixture_path: PathBuf,
    /// 解析结果：错误延迟到 resolve（构造不失败，见模块注释）。
    script: Result<Vec<ScriptStep>, String>,
}

impl std::fmt::Debug for ScriptedFixtureSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Debug 只暴露路径与解析成败，不展开剧本内容（正文可能含路径）。
        formatter
            .debug_struct("ScriptedFixtureSource")
            .field("fixture_path", &self.fixture_path)
            .field("script_ok", &self.script.is_ok())
            .finish()
    }
}

impl ScriptedFixtureSource {
    /// 生产入口：env 未设置（或空白）返回 `None`，完全不影响默认装配链。
    pub fn from_env() -> Option<Self> {
        let value = std::env::var(SCRIPTED_PROVIDER_ENV).ok()?;
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return None;
        }
        Some(Self::from_fixture_path(PathBuf::from(trimmed)))
    }

    /// 测试/嵌入构造：显式夹具路径。
    pub fn from_fixture_path(path: PathBuf) -> Self {
        let script = read_script(&path);
        Self {
            fixture_path: path,
            script,
        }
    }

    /// 夹具文件路径（诊断用）。
    pub fn fixture_path(&self) -> &std::path::Path {
        &self.fixture_path
    }
}

impl ProviderSource for ScriptedFixtureSource {
    fn resolve(
        &self,
        _provider_id: &str,
        _model_id: &str,
        context: Option<&crate::runtime::ProviderRequestContext>,
    ) -> Result<Arc<dyn ModelProvider>, String> {
        let script = self.script.clone()?;
        // `${cwd}` 用会话工作目录替换（TS mock-provider 的 `substituteCwd` 同语义）；
        // 没有上下文（理论上 stdio 路径必有）按空串替换，占位符因此可见而非静默留残。
        let cwd = context
            .map(|context| context.workspace_path.as_str())
            .unwrap_or_default();
        Ok(Arc::new(ScriptedProvider::new(substitute_cwd(script, cwd))))
    }
}

/// 读夹具文件并解析 `script` 数组。所有失败都给稳定文案（不含文件内容）。
fn read_script(path: &std::path::Path) -> Result<Vec<ScriptStep>, String> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        format!(
            "scripted provider 夹具不可读: {} ({error})",
            path.display()
        )
    })?;
    let value: Value = serde_json::from_str(&text).map_err(|_| {
        format!("scripted provider 夹具不是合法 JSON: {}", path.display())
    })?;
    let steps = value.get("script").ok_or_else(|| {
        format!(
            "scripted provider 夹具缺少 script 字段: {}",
            path.display()
        )
    })?;
    let steps = steps.as_array().ok_or_else(|| {
        format!(
            "scripted provider 夹具的 script 不是数组: {}",
            path.display()
        )
    })?;
    steps
        .iter()
        .enumerate()
        .map(|(index, step)| parse_step(index, step))
        .collect()
}

/// 单步映射：`{text}` / `{toolCalls:[{name,input}]}`，其余形状显式报错。
fn parse_step(index: usize, step: &Value) -> Result<ScriptStep, String> {
    let context = |detail: &str| {
        format!("scripted provider 剧本第 {index} 步不合法: {detail}")
    };
    let object = step
        .as_object()
        .ok_or_else(|| context("不是 JSON 对象"))?;
    if let Some(text) = object.get("text") {
        let text = text.as_str().ok_or_else(|| context("text 不是字符串"))?;
        return Ok(ScriptStep::Text(text.to_string()));
    }
    if let Some(calls) = object.get("toolCalls") {
        let calls = calls
            .as_array()
            .ok_or_else(|| context("toolCalls 不是数组"))?;
        let mut parsed = Vec::with_capacity(calls.len());
        for (position, call) in calls.iter().enumerate() {
            let call = call.as_object().ok_or_else(|| {
                context(&format!("toolCalls[{position}] 不是 JSON 对象"))
            })?;
            let name = call
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| context(&format!("toolCalls[{position}].name 不是字符串")))?;
            let input = call.get("input").cloned().unwrap_or(Value::Null);
            parsed.push(ScriptToolCall {
                name: name.to_string(),
                input,
            });
        }
        return Ok(ScriptStep::ToolCalls(parsed));
    }
    Err(context("既没有 text 也没有 toolCalls"))
}

/// `${cwd}` → 实际工作目录（逐字符串替换；数组与对象递归）。
///
/// 与 TS `mock-provider.mts::substituteCwd` 同词同语义：占位符只在**值**里替换，
/// 键名不动；没有命中的字符串原样保留。
fn substitute_cwd(script: Vec<ScriptStep>, cwd: &str) -> Vec<ScriptStep> {
    script
        .into_iter()
        .map(|step| match step {
            ScriptStep::Text(text) => ScriptStep::Text(text.replace(CWD_TOKEN, cwd)),
            ScriptStep::ToolCalls(calls) => ScriptStep::ToolCalls(
                calls
                    .into_iter()
                    .map(|call| ScriptToolCall {
                        name: call.name,
                        input: replace_value(call.input, cwd),
                    })
                    .collect(),
            ),
        })
        .collect()
}

fn replace_value(value: Value, cwd: &str) -> Value {
    match value {
        Value::String(text) => Value::String(text.replace(CWD_TOKEN, cwd)),
        Value::Array(items) => {
            Value::Array(items.into_iter().map(|item| replace_value(item, cwd)).collect())
        }
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, inner)| (key, replace_value(inner, cwd)))
                .collect(),
        ),
        other => other,
    }
}
