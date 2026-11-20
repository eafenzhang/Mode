//! 纯文本视图 + ratatui 渲染。纯文本视图让场景断言不依赖终端后端。

use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::model::TuiModel;

/// 与 TUI 宽度无关的文本视图：header / 会话行 / 输入行 / 状态行。
pub fn view_lines(model: &TuiModel, width: usize) -> Vec<String> {
    let mut lines = Vec::with_capacity(model.sessions.len() + 3);
    lines.push(truncate(
        &format!("Mode · {}", model.workspace),
        width,
    ));
    if model.sessions.is_empty() {
        lines.push(truncate("还没有任务", width));
    } else {
        for (index, session) in model.sessions.iter().enumerate() {
            let marker = if index == model.selected { ">" } else { " " };
            lines.push(truncate(
                &format!(
                    "{marker} {} {}  {}",
                    session.phase.indicator(),
                    session.title,
                    session.last_activity
                ),
                width,
            ));
        }
    }
    lines.push(truncate(&format!("> {}", model.input), width));
    lines.push(truncate(&model.status, width));
    lines
}

pub fn render(model: &TuiModel, frame: &mut Frame) {
    let area = frame.area();
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .split(area);

    let header = Paragraph::new(Line::from(vec![
        Span::styled(
            " Mode ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!("  {}", model.workspace)),
    ]));
    frame.render_widget(header, chunks[0]);

    let items = if model.sessions.is_empty() {
        vec![ListItem::new(Line::from(Span::styled(
            "还没有任务",
            Style::default().fg(Color::DarkGray),
        )))]
    } else {
        model
            .sessions
            .iter()
            .map(|session| {
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{} ", session.phase.indicator()),
                        Style::default().fg(phase_color(session.phase)),
                    ),
                    Span::raw(session.title.clone()),
                    Span::styled(
                        format!("  {}", session.last_activity),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]))
            })
            .collect()
    };
    let list = List::new(items)
        .block(Block::bordered().title("任务"))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    let mut state = ListState::default();
    if !model.sessions.is_empty() {
        state.select(Some(model.selected));
    }
    frame.render_stateful_widget(list, chunks[1], &mut state);

    let input = Paragraph::new(format!("> {}", model.input))
        .block(Block::bordered().title("输入 (Enter 发送 · q 退出)"));
    frame.render_widget(input, chunks[2]);

    frame.render_widget(
        Paragraph::new(model.status.as_str()).style(Style::default().fg(Color::DarkGray)),
        chunks[3],
    );
}

fn phase_color(phase: crate::model::SessionPhase) -> Color {
    use crate::model::SessionPhase;
    match phase {
        SessionPhase::Draft => Color::Gray,
        SessionPhase::Running => Color::Cyan,
        SessionPhase::Waiting => Color::Yellow,
        SessionPhase::Done => Color::Green,
        SessionPhase::Failed => Color::Red,
    }
}

/// 按字符（不是字节）截断，避免中文/emoji 被切坏。
fn truncate(value: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let chars: Vec<char> = value.chars().collect();
    if chars.len() <= width {
        return value.to_string();
    }
    if width == 1 {
        return "…".to_string();
    }
    format!("{}…", chars.into_iter().take(width - 1).collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{SessionPhase, TuiSession};

    #[test]
    fn view_lines_marks_selection_and_phases() {
        let model = TuiModel::with_sessions(
            "ws",
            vec![
                TuiSession::new("a", "迁移 TUI", SessionPhase::Running, "刚刚"),
                TuiSession::new("b", "修复弹窗", SessionPhase::Failed, "1 天"),
            ],
        );
        let lines = view_lines(&model, 120);
        assert_eq!(lines[0], "Mode · ws");
        assert!(lines[1].starts_with("> ● 迁移 TUI"));
        assert!(lines[2].starts_with("  ✗ 修复弹窗"));
        assert_eq!(lines[3], "> ");
        assert_eq!(lines[4], "就绪");
    }

    #[test]
    fn view_lines_handles_empty_sessions_and_unicode_truncation() {
        let model = TuiModel::new("ws");
        let lines = view_lines(&model, 8);
        assert_eq!(lines[1], "还没有任务");
        let long = TuiModel::with_sessions(
            "ws",
            vec![TuiSession::new("a", "这是一个很长的中文标题", SessionPhase::Done, "")],
        );
        let lines = view_lines(&long, 8);
        assert_eq!(lines[1].chars().count(), 8);
        assert!(lines[1].ends_with('…'));
    }
}
