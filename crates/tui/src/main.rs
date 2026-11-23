//! 最小可运行 TUI：连接 agent-server（stdio），订阅 sessions-index 并渲染实时列表。
//! Enter 提交走 `session/send`（见 `submit` 模块）；新建任务用 Ctrl-N。

use std::io;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use mode_tui::{
    apply_agent_notification, parse_error_reply, parse_send_result, prepare_submit, render,
    AgentClient, SendOutcome, SubmitState, TuiModel,
};

fn main() -> io::Result<()> {
    let workspace = std::env::var("MODE_TUI_WORKSPACE")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            std::env::current_dir()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|_| ".".to_string())
        });
    let mut model = TuiModel::new(&workspace);
    let agent = connect(&workspace, &mut model);

    let mut terminal = ratatui::init();
    let mut submit = SubmitState::Idle;
    let result = run(
        &mut terminal,
        &mut model,
        agent.as_ref(),
        &workspace,
        &mut submit,
    );
    ratatui::restore();
    result
}

fn connect(workspace: &str, model: &mut TuiModel) -> Option<AgentClient> {
    let client = match AgentClient::spawn_from_env() {
        Ok(client) => client,
        Err(error) => {
            model.status = format!("Agent 未连接（本地模式）：{error}");
            return None;
        }
    };
    let topic = format!("sessions-index/{workspace}");
    let request = serde_json::json!({
        "topic": topic,
        "connectionId": format!("tui-{}", std::process::id()),
        "clientMode": "desktop-continuous",
    });
    match client.request("v4/conversation/subscribe", request) {
        Ok(_) => {
            model.status = "已连接 Agent · ↑/↓ 选择 · Ctrl-N 新建 · Ctrl-Q 退出".to_string();
        }
        Err(error) => {
            model.status = format!("Agent 订阅失败：{error}");
        }
    }
    Some(client)
}

fn run(
    terminal: &mut ratatui::DefaultTerminal,
    model: &mut TuiModel,
    agent: Option<&AgentClient>,
    workspace: &str,
    submit: &mut SubmitState,
) -> io::Result<()> {
    loop {
        if let Some(agent) = agent {
            while let Some(message) = agent.try_recv() {
                // 先看是不是我们发出的 session/send 回执；不是再交给通知归约。
                if !apply_send_reply(model, submit, &message) {
                    apply_agent_notification(model, &message, now_ms());
                }
            }
        }
        terminal.draw(|frame| render(model, frame))?;
        if !event::poll(Duration::from_millis(200))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('q') => break,
                KeyCode::Char('n') => {
                    if let Some(agent) = agent {
                        let params = serde_json::json!({
                            "workspace": {
                                "workspacePath": workspace,
                                "workspaceKey": workspace,
                            },
                        });
                        match agent.request("session/create", params) {
                            Ok(_) => model.status = "已请求新建任务".to_string(),
                            Err(error) => model.status = format!("新建任务失败：{error}"),
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        match key.code {
            KeyCode::Esc => {
                if model.input.is_empty() {
                    break;
                }
                model.input.clear();
            }
            KeyCode::Up => model.select_previous(),
            KeyCode::Down => model.select_next(),
            KeyCode::Enter => {
                // 输入行**不在这里清空**：等回执回来再清，RPC 失败时用户的字还在。
                let session_id = model.selected_session().map(|item| item.id.clone());
                match prepare_submit(submit, session_id.as_deref(), &model.input, &input_id()) {
                    Err(rejection) => model.status = rejection.status_message().to_string(),
                    Ok(params) => match agent {
                        // 没有 agent：本地回显保留，但状态栏必须说清没有发出去。
                        None => {
                            if let Some(text) = model.submit_input() {
                                model.status = format!("已记录（Agent 未连接，未发送）：{text}");
                            }
                        }
                        Some(agent) => match agent.request("session/send", params) {
                            Ok(request_id) => {
                                model.status = "正在提交…".to_string();
                                *submit = SubmitState::InFlight {
                                    request_id,
                                    input_id: input_id(),
                                    session_id: session_id.unwrap_or_default(),
                                };
                            }
                            Err(error) => {
                                model.status = format!("提交失败（写入失败）：{error}");
                            }
                        },
                    },
                }
            }
            KeyCode::Backspace => model.backspace(),
            KeyCode::Char(value) => model.push_char(value),
            _ => {}
        }
    }
    Ok(())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// 处理一次 `session/send` 回执。返回 `false` = 不是本次提交的回执。
///
/// 成功时清空输入行并退出在途状态；失败时**保留**输入行（用户的字不能丢）。
fn apply_send_reply(
    model: &mut TuiModel,
    submit: &mut SubmitState,
    message: &serde_json::Value,
) -> bool {
    if !submit.is_in_flight() {
        return false;
    }
    let id = message.get("id").and_then(serde_json::Value::as_u64);
    if id != submit.request_id() {
        return false;
    }
    let outcome: SendOutcome = if let Some(error) = message.get("error") {
        parse_error_reply(error)
    } else {
        parse_send_result(message.get("result").unwrap_or(&serde_json::Value::Null))
    };
    model.status = outcome.status_message();
    if outcome.is_accepted() {
        // 受理后才清空：这条输入已经进了 agent 的队列。
        model.input.clear();
    }
    *submit = SubmitState::Idle;
    true
}

/// 提交幂等键：同一进程内单调递增，重试共用同一个 id。
fn input_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!(
        "tui-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    )
}
