# TUI 改 ratatui（LT-8）

状态：v1（2026-11-21，第一段纵切）。上位：`docs/plans/2026-10-29-full-rust-rewrite.md` §W5；
行为基线：`docs/plans/2026-10-18-m3-tui-status.md` 的 33 个场景清单。
不做第二套产品客户端：TUI 只是 Agent 客户端的终端呈现，状态仍归 Agent/Host。

## 1. 范围

| 层 | 归属 | 说明 |
| --- | --- | --- |
| 视图模型 | `crates/tui/src/model.rs` | 会话列表、选中项、输入缓冲、状态行；纯数据，无 IO |
| 渲染 | `crates/tui/src/render.rs` | `view_lines` 纯文本视图 + ratatui 渲染；中文按字符截断 |
| 入口 | `crates/tui/src/main.rs` | crossterm 事件循环骨架；Enter 仅本地回显，尚未接 Agent |
| Agent 适配 | 后续 | 复用 `TuiSubscribeSessionEvents` / 权限 / 提交等 Host 语义，不新建运行时 |

## 2. 第一段纵切（已交付）

- `TuiModel`：会话集合、选中环回、Unicode 安全输入/退格、trim 后提交。
- `view_lines`：header / 会话行（phase 指示符 + 选中标记）/ 输入行 / 状态行，
  按字符截断（不切坏中文/emoji）。
- `render`：ratatui `List` + `Paragraph`，phase 颜色与选中反色。
- 可运行二进制：`cargo run -p mode-tui`，`↑/↓` 选择、`Enter` 本地回显、`q/Esc` 退出。
- 测试：5 条（选择环回、集合收缩夹紧、Unicode 输入、空态、截断）。

验收：`cargo test -p mode-tui` 全绿；`cargo run -p mode-tui` 在 Windows Terminal /
macOS Terminal / Linux tty 能绘制并退出。

## 3. 后续切片（按 33 场景）

1. **A1/A2/A3**：接 Agent 会话事件（启动、恢复、turn 完成），替换本地回显。
2. **A5/A6**：排队输入与输入历史（busy 时入队、上下键召回）。
3. **B 组**：流式文本/工具增量（复用事件归约，不重写投影）。
4. **C 组（审批/权限）**：权限请求面板与键盘确认。
5. **D 组（工作流/子代理/侧栏）**：只读查询面接入。
6. 主题、IME、剪贴板等终端差异场景按清单逐项人工验收。

## 4. 边界

- 不在 TUI 里复制会话状态机：事件订阅是唯一输入，提交走 Host 服务。
- 不引入 Node/Electron 依赖；ratatui + crossterm 纯终端。
- 任何新增快捷键/面板先补对应场景编号与测试，再改渲染。
