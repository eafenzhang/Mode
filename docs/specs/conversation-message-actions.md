# Spec：对话消息动作行与时间戳

状态：2026-10-28 实现。关联：
[user-profile-display.md](./user-profile-display.md)（左下角资料展示）、
`packages/ui/src/v4/ConversationRowView.tsx`。

## 目的

消息动作（复制、编辑、反馈、分叉）和消息时间在桌面端不能只靠 hover 才能发现：
远控、触屏和无指针设备没有稳定的 hover 入口。用户消息也需要和助手消息一样显示发送时间，
避免同一轮里只有一侧有时间。

## 产品规则

1. **动作行常驻**：用户消息和助手消息的动作行始终可见，不再使用
   `opacity-0` + `group-hover/...` 的透明度门控；保留 focus 可访问性。
2. **用户消息时间戳**：用户行在动作行末尾显示 `row.createdAt` 的派生时间，格式复用
   `formatMessageTimeLabel`；与助手行一致，同一天只显示时间，跨天显示日期/时间。
3. **助手消息时间戳**：保持现有行为，继续在完成态动作行末尾显示。
4. **只读派生**：时间只从 `row.createdAt` 计算，不新增 renderer 状态、不写回协议、
   不改变持久化。
5. **顺序稳定**：用户行按复制 → 编辑 → 时间渲染；助手行按复制 → 反馈 → 分叉 → hook →
   时间渲染，时间始终在动作行末尾。
6. **平台一致**：桌面、Web、手机远控使用同一套常驻语义；不做「远控特判常显、桌面 hover」
   的分叉。

## 回复速度

1. **数据源**：`session/debug` 的 round 新增可选字段 `prefillDurationMs` 与
   `prefillTokensPerSecond`；旧 CLI 不返回时只是不显示速度，不影响消息渲染。
2. **prefill 定义**：未命中缓存的输入 token / TTFT（`timeToFirstContentMs`）。
   全缓存直出或 TTFT 未知时 `prefillTokensPerSecond` 为 null，不显示 `↑`。
3. **decode 定义**：沿用 `tokensPerSecond`（输出 token / 首 token 后的生成时长）。
4. **按轮匹配**：取该轮 `[startedAt, endedAt]` 内最后一次带速度的 round；迟到/窗口外
   的 round 不参与。已创建轮不因后续重订阅重算历史速度。
5. **拉取策略**：历史打开时补一次 debug 快照（最多重试 2 次），运行中的轮每秒跟进，
   轮完成后停止；不新增持久化状态。
6. **展示**：动作行在时间戳之前显示 `↑1,200.4 tok/s` / `↓88.6 tok/s`，数字按当前
   locale 格式化；没有速度时整段省略。

## 状态所有者

| 状态 | 所有者 | 说明 |
| --- | --- | --- |
| 消息创建时间 | V4 row 投影 `row.createdAt` | CLI 事实，UI 只读 |
| 动作可用性 | row.actions / 调用方回调 | UI 不猜测能力 |
| 复制成功态 | `CopyRowAction` 本地 1200ms 状态 | 纯展示，不持久化 |

## 验收

1. 桌面端不 hover 任意消息也能看到复制/编辑/反馈/分叉入口。
2. 用户消息和助手消息都显示时间；同一天只显示时间，跨天显示日期。
3. 消息时间在动作行末尾，不插入动作按钮之间。
4. 渲染结果不再包含 `opacity-0`、`group-hover/user-row`、
   `group-hover/assistant-turn`、`group-hover/assistant-row` 的动作行样式。
5. 中文/英文、深浅主题、桌面/窄屏下动作行不溢出、不遮挡正文。
