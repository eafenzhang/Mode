# Spec：TUI 输入提交（LT-8）

状态：**已接（2026-11-23）**——`crates/tui/src/submit.rs`（10 条测试）+ `main.rs` 接线。
Enter 不再只做本地回显。

## 1. 之前是什么样

`main.rs` 的 Enter 分支只做本地回显：

```rust
model.status = format!("已提交（本地回显，尚未发送）：{text}");
```

用户按 Enter 后看起来"发出去了"，实际没有任何 RPC。这是**用户可见的 bug**，不是缺失
功能——状态栏写着"已提交"，人不会再去查。

## 2. 现在的链路

```text
Enter
  ├─ prepare_submit(在途? / 选中会话? / 输入非空?)   ← 三条拒绝各有文案
  ├─ agent.request("session/send", {sessionId, content, inputId})
  │     └─ Ok(request_id) → SubmitState::InFlight{request_id, input_id, session_id}
  └─ 状态栏「正在提交…」

回执到达（try_recv）
  ├─ apply_send_reply 按 RPC id 对齐；不是本次回执 → 交给通知归约
  ├─ accepted:true  → 状态栏「已提交，等待模型响应…」+ **清空输入行**
  └─ 其余            → 状态栏「提交失败：…」+ **保留输入行**
```

## 3. 三条语义（各自对应一个真会犯的错）

### 3.1 没有选中会话就不能发

TUI 的输入行是全局的，但 `session/send` 必须带 `sessionId`。空列表时发出去只会拿到
agent 的 `unknown session`——不如在本地就拦住并提示"先按 Ctrl-N 新建任务"。

### 3.2 `inputId` 由客户端生成且稳定

它是 agent 侧的**幂等键**。重复 Enter（UI 卡顿、终端重复投递）必须带同一个 id，
否则同一条输入会被受理两次。所以 `input_id()` 是进程内单调计数器，
`SubmitState::InFlight` 把它一起存着。

### 3.3 回执是「受理」不是「完成」

`accepted:true` 只表示进了队列；turn 的失败走事件流的 `turn.failed`。状态栏因此写
**「已提交，等待模型响应…」**，绝不写"已完成"。写成"完成"会让用户以为模型已经答完。

## 4. 输入行什么时候清空

**只在受理之后。** 这是本模块最容易写错的地方：如果在按 Enter 时就 `submit_input()`
清空（原实现的路径），RPC 失败时用户的字就没了——尤其在长输入上体验极差。

没有 agent 时（`AgentClient::spawn` 失败）保留本地回显，但文案必须说清没有发出去：
「已记录（Agent 未连接，未发送）」。

## 5. `accepted` 缺字段按拒绝处理

`parse_send_result` 只在 `accepted === true` 时判成功。缺字段 / `false` / `"true"`
（字符串）都按拒绝走。把缺字段当成功会让失败的提交在状态栏显示成"已提交"，
用户永远等不到响应也不知道为什么。

## 6. 不替用户选模型

`build_send_params` 只带三个必需字段：`sessionId` / `content` / `inputId`。
`modelSelection` 等可选字段整个缺席，由 agent 用会话当前值补齐——TUI 没有模型选择 UI，
替用户填反而会覆盖他在桌面端刚改的选择。

## 7. 验收

`cargo test -p mode-tui --lib`（24 条，其中 submit 模块 10 条）：

- 无选中会话 / 空白会话 id → `NoSession`；
- 纯空白输入 → `EmptyInput`（不把空白发给 agent）；
- 在途时第二次 Enter → `InFlight`（不重复受理）；
- params 恰好三个字段且 `content` 已 trim；
- `accepted:true` → `Accepted{revision}`，文案是「已提交，等待模型响应…」；
- 缺字段 / `false` / `"true"` / `null` → 都是拒绝；
- 拒绝文案优先 `message`，其次 `error`，最后兜底；
- JSON-RPC `error` 对象原样透出；
- 每种拒绝都有非空文案。

## 8. 未做

- **流式渲染**：turn 的过程事件（assistant delta / 工具调用）仍未进 TUI，状态栏只有
  "等待模型响应…"。这是 TUI 的大头，需要把 `v4/conversation/frame` 的 conversation
  帧（不是 sessions-index 帧）也接进 `agent_wire`。
- **没有 agent 时的重连**：`AgentClient::spawn` 失败后没有重试路径。
- **取消**：在途提交没有 Ctrl-C 中断（agent 侧的 `PromiseCancel` 未接）。
