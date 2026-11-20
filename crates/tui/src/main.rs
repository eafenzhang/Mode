//! 最小可运行 TUI：连接 agent-server（stdio），订阅 sessions-index 并渲染实时列表。
//! 输入提交仍未接（Enter 只做本地回显）；新建任务用 Ctrl-N。

use std::io;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use mode_tui::{apply_agent_notification, render, AgentClient, TuiModel};

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
    let result = run(&mut terminal, &mut model, agent.as_ref(), &workspace);
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
) -> io::Result<()> {
    loop {
        if let Some(agent) = agent {
            while let Some(message) = agent.try_recv() {
                apply_agent_notification(model, &message, now_ms());
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
                if let Some(text) = model.submit_input() {
                    // 发送面（session/send）在后续切片接入；先保留本地回显验证输入链路。
                    model.status = format!("已提交（本地回显，尚未发送）：{text}");
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
