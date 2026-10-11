# IM 机器人绑定菜单的对话展示（无昵称可区分）

状态：已拍板（2026-10-10）。关联：[im-bot-remote-workspace-binding.md](./im-bot-remote-workspace-binding.md)（绑定语义与失败可见性，本文只管展示）。

## 背景

实机截图：「IM 机器人」子菜单里同一 bot 出现「解绑」+「绑定」两行，可见部分逐字相同，
被误判为重复条目。查 `bot-state.v3.json` 属实为**两条不同对话**（一条 `mode: task` 绑在
当前会话、一条 draft 未绑），但叠加三个展示缺陷后不可区分：

1. 两条对话均无 `conversationLabel`（昵称），`describeConversation` 回退显示**裸 conversationId**；
2. 菜单宽 `w-80`，标签在差异字符出现之前就已截断（`私聊 · o9cq80…` 与 `私聊 · 8b4388…`
   的可见前缀相同）；
3. 两行右侧显示的 bot id 相同（本来就是同一 bot）。

## 产品规则

- **有 `conversationLabel` 时展示不变**（`私聊 · {昵称}` / `群聊 · {id}`）。
- **label 缺失时**，裸 id 降级为可区分 token：取 `@` 前本地段的**最后 6 字符** + `@域`，
  前置省略号——`…9be073@im.bot` / `…mH5Ldc@im.wechat`；本地段 ≤6 字符或无 `@` 时
  按同样规则处理整体（过短不缩）。token 必须让同一 bot 的两条无昵称对话在
  「解绑：…（私聊 · 」前缀之后立刻分叉。
- **悬停可见全键**：绑定/解绑行的文案区带 `title` = 完整 conversationId，诊断不丢信息。
- 解绑/绑定的**对话粒度语义不变**（本 spec 不改任何绑定/状态机/键格式）；群聊子菜单
  （多候选对话先选后绑）走同一降级函数。
- i18n 文案结构不变：降级 token 复用既有 `{name}`/`{id}` 插槽，不新增语言键。

## 状态所有者

- 展示降级为**纯函数** `shortenConversationId`（`packages/ui/src/lib/botConversationDisplay.ts`），
  唯一实现点；`botBindingMenu.tsx` 的 `describeConversation` 是唯一消费点。
- 数据来源不变：解绑行 ← `botTaskBindingsStore`（`listBotTaskBindings`，mode=task 的对话）；
  绑定行 ← `botsService.listBotConversations`（未绑对话）。

## 验收场景

1. 单测（`packages/ui/test/botBindingMenuDisplay.test.ts`）：用实机两条真实 id 断言
   降级 token 共同前缀 ≤ `…`（经「私聊 · 」模板后可见前缀仍分叉）；短 id 不缩；
   本地段+域的拼接形状正确。
2. 带 label 的对话展示逻辑不经过降级函数（行为不变，由实现保证 + 实机确认）。
3. `pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed` 通过。
4. 实机：同一 bot 的「解绑/绑定」两行在默认菜单宽度下肉眼可区分。

## 非目标

- 不自动回写/补全 `conversationLabel`；不改 conversationKey 格式与绑定存储；
- 不合并/隐藏「解绑+绑定」双行（它们语义正确）；不动 en/fa 文案键。
