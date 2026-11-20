//! TUI 视图模型：只保存渲染所需事实，不持有 Agent/终端句柄。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionPhase {
    Draft,
    Running,
    Waiting,
    Done,
    Failed,
}

impl SessionPhase {
    pub fn indicator(self) -> &'static str {
        match self {
            Self::Draft => "○",
            Self::Running => "●",
            Self::Waiting => "◐",
            Self::Done => "✓",
            Self::Failed => "✗",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiSession {
    pub id: String,
    pub title: String,
    pub phase: SessionPhase,
    pub last_activity: String,
}

impl TuiSession {
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        phase: SessionPhase,
        last_activity: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            phase,
            last_activity: last_activity.into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TuiModel {
    pub workspace: String,
    pub sessions: Vec<TuiSession>,
    pub selected: usize,
    pub input: String,
    pub status: String,
}

impl TuiModel {
    pub fn new(workspace: impl Into<String>) -> Self {
        Self {
            workspace: workspace.into(),
            status: "就绪".to_string(),
            ..Self::default()
        }
    }

    pub fn with_sessions(workspace: impl Into<String>, sessions: Vec<TuiSession>) -> Self {
        let mut model = Self::new(workspace);
        model.set_sessions(sessions);
        model
    }

    pub fn set_sessions(&mut self, sessions: Vec<TuiSession>) {
        self.sessions = sessions;
        if self.sessions.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.sessions.len() {
            self.selected = self.sessions.len() - 1;
        }
    }

    pub fn selected_session(&self) -> Option<&TuiSession> {
        self.sessions.get(self.selected)
    }

    pub fn select_next(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.sessions.len();
    }

    pub fn select_previous(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        self.selected = if self.selected == 0 {
            self.sessions.len() - 1
        } else {
            self.selected - 1
        };
    }

    pub fn push_char(&mut self, value: char) {
        if !value.is_control() {
            self.input.push(value);
        }
    }

    pub fn backspace(&mut self) {
        self.input.pop();
    }

    /// 提交输入并清空；空输入返回 `None`，避免把纯空白发送给 Agent。
    pub fn submit_input(&mut self) -> Option<String> {
        let text = self.input.trim().to_string();
        self.input.clear();
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sessions() -> Vec<TuiSession> {
        vec![
            TuiSession::new("a", "第一项", SessionPhase::Running, "1 分钟"),
            TuiSession::new("b", "第二项", SessionPhase::Done, "2 天"),
            TuiSession::new("c", "第三项", SessionPhase::Failed, "3 天"),
        ]
    }

    #[test]
    fn selection_wraps_in_both_directions() {
        let mut model = TuiModel::with_sessions("ws", sessions());
        assert_eq!(model.selected_session().map(|item| item.id.as_str()), Some("a"));
        model.select_previous();
        assert_eq!(model.selected, 2);
        model.select_next();
        assert_eq!(model.selected, 0);
    }

    #[test]
    fn set_sessions_clamps_selection() {
        let mut model = TuiModel::with_sessions("ws", sessions());
        model.selected = 2;
        model.set_sessions(sessions().into_iter().take(1).collect());
        assert_eq!(model.selected, 0);
        model.set_sessions(Vec::new());
        assert_eq!(model.selected, 0);
        assert!(model.selected_session().is_none());
    }

    #[test]
    fn input_is_unicode_safe_and_trims_on_submit() {
        let mut model = TuiModel::new("ws");
        for value in "你好 @文件".chars() {
            model.push_char(value);
        }
        model.backspace();
        assert_eq!(model.input, "你好 @文");
        model.backspace();
        assert_eq!(model.submit_input().as_deref(), Some("你好 @"));
        assert!(model.input.is_empty());
        assert_eq!(model.submit_input(), None);
        model.input = "   ".to_string();
        assert_eq!(model.submit_input(), None);
    }
}
