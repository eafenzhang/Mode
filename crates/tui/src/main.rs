//! 最小可运行 TUI：验证视图模型与渲染边界；Agent/会话接线在后续纵切。

use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use mode_tui::{render, TuiModel};

fn main() -> std::io::Result<()> {
    let mut terminal = ratatui::init();
    let mut model = TuiModel::new("workspace");
    model.status = "骨架模式：Enter 只做本地回显，尚未接 Agent".to_string();
    let result = run(&mut terminal, &mut model);
    ratatui::restore();
    result
}

fn run(terminal: &mut ratatui::DefaultTerminal, model: &mut TuiModel) -> std::io::Result<()> {
    loop {
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
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => break,
            KeyCode::Up => model.select_previous(),
            KeyCode::Down => model.select_next(),
            KeyCode::Enter => {
                if let Some(text) = model.submit_input() {
                    model.status = format!("已提交（本地回显）：{text}");
                }
            }
            KeyCode::Backspace => model.backspace(),
            KeyCode::Char(value) => model.push_char(value),
            _ => {}
        }
    }
    Ok(())
}
