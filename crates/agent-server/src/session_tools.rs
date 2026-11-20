//! 会话级工具：TodoWrite / TodoRead / ExitPlanMode（C4 T6 补齐）。
//!
//! 观察基线：`.plan/accept-c4` 的 oracle 黑盒探针（工具声明、结果 JSON、todo 表行、
//! `session/read` 的 todos/todoGroups、`v4/conversation/plans`）。Todo 的持久化事实源
//! 是 `session-store` 的 `todo` 表；ExitPlanMode 只产出一条普通 tool part，plans 查询
//! 按工具名投影。

use std::sync::{Arc, Mutex};

use agent_core::{PluginToolError, PluginToolSource};
use agent_protocol::snapshot::{TodoGroup, TodoGroupSource, TodoItem, TodoPriority, TodoStatus};
use serde_json::{json, Value};
use session_store::{SessionStore, TodoRecord};

/// 会话工具声明与执行（每个 turn 由 runtime 注入同一份 cache）。
pub struct SessionToolSource {
    store: Arc<SessionStore>,
    session_id: String,
    todos: Arc<Mutex<Vec<TodoItem>>>,
}

impl SessionToolSource {
    pub fn new(
        store: Arc<SessionStore>,
        session_id: String,
        todos: Arc<Mutex<Vec<TodoItem>>>,
    ) -> Self {
        Self {
            store,
            session_id,
            todos,
        }
    }
}

impl PluginToolSource for SessionToolSource {
    fn list_tools(&self) -> Result<Vec<agent_core::PluginToolDeclaration>, String> {
        Ok(vec![
            agent_core::PluginToolDeclaration {
                name: "TodoWrite".to_string(),
                description: TODO_WRITE_DESCRIPTION.to_string(),
                input_schema: todo_write_schema(),
                risk_level: Some("low".to_string()),
                side_effect_scope: Some("session".to_string()),
                budget_strategy: Some("truncate".to_string()),
                read_only: Some(true),
                display_json: None,
            },
            agent_core::PluginToolDeclaration {
                name: "TodoRead".to_string(),
                description: "Read the current session todo list".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false,
                    "$schema": "https://json-schema.org/draft/2020-12/schema"
                }),
                risk_level: Some("low".to_string()),
                side_effect_scope: Some("none".to_string()),
                budget_strategy: Some("truncate".to_string()),
                read_only: Some(true),
                display_json: None,
            },
            agent_core::PluginToolDeclaration {
                name: "ExitPlanMode".to_string(),
                description: EXIT_PLAN_MODE_DESCRIPTION.to_string(),
                input_schema: exit_plan_mode_schema(),
                risk_level: Some("medium".to_string()),
                side_effect_scope: Some("session".to_string()),
                budget_strategy: Some("truncate".to_string()),
                read_only: Some(true),
                display_json: None,
            },
        ])
    }

    fn call_tool(&self, tool_name: &str, input: &Value) -> Result<Value, PluginToolError> {
        match tool_name {
            "TodoWrite" => self.todo_write(input),
            "TodoRead" => self.todo_read(),
            "ExitPlanMode" => self.exit_plan_mode(input),
            other => Err(PluginToolError::tool_execution_failed(format!(
                "Unknown session tool: {other}"
            ))),
        }
    }
}

impl SessionToolSource {
    fn todo_write(&self, input: &Value) -> Result<Value, PluginToolError> {
        let entries = input
            .get("todos")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid("todos must be an array"))?;
        let mut todos = Vec::with_capacity(entries.len());
        let mut in_progress = 0usize;
        for entry in entries {
            let object = entry
                .as_object()
                .ok_or_else(|| invalid("each todo must be an object"))?;
            let content = object
                .get("content")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| invalid("todo.content is required"))?;
            let status = match object.get("status").and_then(Value::as_str) {
                Some("pending") => TodoStatus::Pending,
                Some("in_progress") => TodoStatus::InProgress,
                Some("completed") => TodoStatus::Completed,
                _ => return Err(invalid("todo.status is invalid")),
            };
            let priority = match object.get("priority").and_then(Value::as_str) {
                Some("high") => TodoPriority::High,
                Some("medium") => TodoPriority::Medium,
                Some("low") => TodoPriority::Low,
                _ => return Err(invalid("todo.priority is invalid")),
            };
            if status == TodoStatus::InProgress {
                in_progress += 1;
            }
            todos.push(TodoItem {
                content: content.to_string(),
                status,
                priority,
            });
        }
        if in_progress > 1 {
            return Err(invalid("at most one todo may be in_progress"));
        }
        let old = self
            .todos
            .lock()
            .map_err(|_| invalid("todo state poisoned"))?
            .clone();
        let now = now_ms();
        let records: Vec<TodoRecord> = todos
            .iter()
            .enumerate()
            .map(|(position, todo)| TodoRecord {
                content: todo.content.clone(),
                status: status_text(todo.status).to_string(),
                priority: priority_text(todo.priority).to_string(),
                position: position as i64,
                time_created: now,
                time_updated: now,
            })
            .collect();
        self.store
            .replace_todos(&self.session_id, &records)
            .map_err(|error| invalid(&error.to_string()))?;
        *self
            .todos
            .lock()
            .map_err(|_| invalid("todo state poisoned"))? = todos.clone();
        let pending = todos
            .iter()
            .filter(|todo| todo.status == TodoStatus::Pending)
            .count();
        let in_progress = todos
            .iter()
            .filter(|todo| todo.status == TodoStatus::InProgress)
            .count();
        let completed = todos
            .iter()
            .filter(|todo| todo.status == TodoStatus::Completed)
            .count();
        let old_json = serde_json::to_string(&old).unwrap_or_else(|_| "[]".to_string());
        let todos_json = serde_json::to_string(&todos).unwrap_or_else(|_| "[]".to_string());
        let summary = format!(
            "{{\"total\":{},\"pending\":{},\"inProgress\":{},\"completed\":{}}}",
            todos.len(),
            pending,
            in_progress,
            completed
        );
        let content =
            format!("{{\"oldTodos\":{old_json},\"todos\":{todos_json},\"summary\":{summary}}}");
        Ok(json!({ "content": content }))
    }

    fn todo_read(&self) -> Result<Value, PluginToolError> {
        let todos = self
            .todos
            .lock()
            .map_err(|_| invalid("todo state poisoned"))?
            .clone();
        let todos_json = serde_json::to_string(&todos).unwrap_or_else(|_| "[]".to_string());
        Ok(json!({ "content": format!("{{\"todos\":{todos_json}}}") }))
    }

    fn exit_plan_mode(&self, input: &Value) -> Result<Value, PluginToolError> {
        let plan = input
            .get("plan")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| invalid("plan is required"))?;
        Ok(json!({ "content": format!("Plan submitted for approval.\n\n{plan}") }))
    }
}

/// 冷/热快照共用的投影：`todos` + 单个 `source:"session"` 分组。
pub fn todo_snapshot(items: &[TodoItem]) -> (Vec<TodoItem>, Vec<TodoGroup>) {
    if items.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let now = now_ms() as u64;
    (
        items.to_vec(),
        vec![TodoGroup {
            id: "session-todos".to_string(),
            source: TodoGroupSource::Session,
            goal_iteration: None,
            target_id: None,
            started_at: Some(now),
            updated_at: Some(now),
            todos: items.to_vec(),
        }],
    )
}

/// `session-store` 行 → 协议条目。
pub fn items_from_records(records: &[TodoRecord]) -> Vec<TodoItem> {
    records
        .iter()
        .filter_map(|record| {
            let status = match record.status.as_str() {
                "pending" => TodoStatus::Pending,
                "in_progress" => TodoStatus::InProgress,
                "completed" => TodoStatus::Completed,
                _ => return None,
            };
            let priority = match record.priority.as_str() {
                "high" => TodoPriority::High,
                "medium" => TodoPriority::Medium,
                "low" => TodoPriority::Low,
                _ => return None,
            };
            Some(TodoItem {
                content: record.content.clone(),
                status,
                priority,
            })
        })
        .collect()
}

fn status_text(status: TodoStatus) -> &'static str {
    match status {
        TodoStatus::Pending => "pending",
        TodoStatus::InProgress => "in_progress",
        TodoStatus::Completed => "completed",
    }
}

fn priority_text(priority: TodoPriority) -> &'static str {
    match priority {
        TodoPriority::High => "high",
        TodoPriority::Medium => "medium",
        TodoPriority::Low => "low",
    }
}

fn invalid(message: &str) -> PluginToolError {
    PluginToolError::tool_execution_failed(format!("Invalid TodoWrite input: {message}"))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

const TODO_WRITE_DESCRIPTION: &str = "Create and update a task list for the current session. The list is rendered to the user as your working plan.\n\n- Each todo has `content`, `status` (\"pending\" | \"in_progress\" | \"completed\"), and `priority` (\"high\" | \"medium\" | \"low\").\n- Send the full list each call; it replaces the previous one.\n- Keep one item `in_progress` at a time and mark it `completed` when done.";

fn todo_write_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "todos": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "content": { "type": "string", "minLength": 1, "description": "Brief description of the task" },
                        "status": { "type": "string", "enum": ["pending", "in_progress", "completed"], "description": "Current status of the task" },
                        "priority": { "type": "string", "enum": ["high", "medium", "low"], "description": "Priority level of the task" }
                    },
                    "required": ["content", "status", "priority"],
                    "additionalProperties": false
                },
                "description": "The complete updated todo list. At most one item may be in_progress at a time."
            }
        },
        "required": ["todos"],
        "additionalProperties": false,
        "$schema": "https://json-schema.org/draft/2020-12/schema"
    })
}

fn exit_plan_mode_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "plan": { "type": "string", "minLength": 1, "maxLength": 20000, "description": "The implementation plan to present to the user for approval." },
            "allowedPrompts": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "tool": { "type": "string", "enum": ["Bash"], "description": "The tool this prompt applies to" },
                        "prompt": { "type": "string", "description": "Semantic description of the action, e.g. \"run tests\", \"install dependencies\"" }
                    },
                    "required": ["tool", "prompt"],
                    "additionalProperties": false
                },
                "description": "Prompt-based permissions needed to implement the plan. These describe categories of actions rather than specific commands."
            }
        },
        "required": ["plan"],
        "additionalProperties": {},
        "$schema": "https://json-schema.org/draft/2020-12/schema"
    })
}

const EXIT_PLAN_MODE_DESCRIPTION: &str = "Use this tool when you are in plan mode and have finished writing your plan and are ready for user approval.\n\n## How This Tool Works\n- You should have already explored the codebase and finalized the plan you want the user to review\n- This tool DOES take the plan content as the required plan parameter in Mode\n- Pass the complete plan in the plan field; the user will review that content before approving implementation\n- This tool simply signals that you're done planning and ready for the user to review and approve\n- The user will see the contents of the plan parameter when they review it\n\n## When to Use This Tool\nIMPORTANT: Only use this tool when the task requires planning the implementation steps of a task that requires writing code. For research tasks where you're gathering information, searching files, reading files or in general trying to understand the codebase - do NOT use this tool.\n\n## Before Using This Tool\nEnsure your plan is complete and unambiguous:\n- If you have unresolved questions about requirements or approach, use AskUserQuestion before finalizing your plan\n- Once your plan is finalized, use THIS tool to request approval\n\n**Important:** Do NOT use AskUserQuestion to ask \"Is this plan okay?\" or \"Should I proceed?\" - that's exactly what THIS tool does. ExitPlanMode inherently requests user approval of your plan.";
