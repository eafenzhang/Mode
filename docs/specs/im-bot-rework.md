# IM Bot 按 MyAgents 重做（通道固定 4 个）

状态：已被 [im-bot-agent-channel-aplus.md](./im-bot-agent-channel-aplus.md) 取代（2026-10-20，用户确认 A+：Agent 拥有 Channel、微信仅私聊、保留现有卡片 UI、运行时留在 Node services）。本文件保留为决策历史。
用户指令（2026-10-15）：「imbot 根据 myagents 项目的 bot 机器人全部重做，但依然只支持当前的 4 通道。」

关联：[im-bot-onboarding.md](./im-bot-onboarding.md)（接入现状，本 spec 的 B3 会**推翻**其 C2 约束）、
[im-bot-group-chat-alignment.md](./im-bot-group-chat-alignment.md)（群聊 G1–G10 达标定义，本 spec 沿用其口径）、
[bot-reply-granularity.md](./bot-reply-granularity.md)（回复粒度，已批准）、
[bot-state-ownership.md](./bot-state-ownership.md)（状态所有权不变式）、
[bot-binding-indicators.md](./bot-binding-indicators.md)（标识呈现，本轮已定）、
[im-bot-remote-workspace-binding.md](./im-bot-remote-workspace-binding.md)、
[bot-outbound-target.md](./bot-outbound-target.md)、[bot-outbound-delivery.md](./bot-outbound-delivery.md)。

## 参照来源与许可边界

- 唯一允许的行为来源：团队已完成的黑盒分析 `D:\gas-claw\MyAgents-分析报告.md` §5（该报告为团队自写）。
- MyAgents 本体为 **AGPL-3.0**（`D:\gas-claw\MyAgents-ref`）：**禁止读取其实现源码**，也禁止把内容写进
  spec / 代码。只对齐**行为与步骤集合**（含占位文案这类观感细节），不翻译、不复制其代码。
- 本仓库上游衍生文件的边界同 `docs/specs/clean-room-policy.md`：`packages/services/src/bots/` 下
  多个文件与 `upstream/zcode-baseline` 逐字节相同或高度同源（`replyFormatter.ts`、`botsService.ts`、
  `channelRuntime.ts`、`config.ts`、`repo.ts`、`statusFormatting.ts`、`botConfigHelpers.ts`）。
  **禁止把它们当作编写参考**；「完整功能对齐」= clean-room 重实现，不是移植接线。
  自研文件（基线中不存在）：`outboundDelivery.ts`、`groupHistory.ts`、`groupPrompt.ts`、`messageQueue.ts`。

## 现状盘点（避免与本 spec 重复造轮子）

| 面       | 现状                                                                           | 来源                                                 |
| -------- | ------------------------------------------------------------------------------ | ---------------------------------------------------- |
| 接入     | 四通道卡片式；飞书手填已补、微信两态已拆、三段状态行已接线、录入即验证已接     | `im-bot-onboarding.md`（R3-1/2/4/5 已实现）          |
| 通道集合 | 产品面固定 `weixin / wecom / feishu / dingtalk`（`packages/ui/src/botsUi.ts`） | 本 spec 的硬约束 C1                                  |
| 回复粒度 | 四档 + provider 矩阵；企微默认 `streaming_card`，其余 `assistant_changes`      | `bot-reply-granularity.md`（已批准）                 |
| 回复流式 | 默认档为「按句边界分段」流式（delta 累积），非逐字打字机                       | `botsService.ts` + `providers/streamingText.ts`      |
| 群聊     | 仅自研 `groupHistory.ts`（内存、重启即丢）+ `groupPrompt.ts`；@ 只解析成布尔   | `im-bot-group-chat-alignment.md` 现状列              |
| 变更摘要 | 只有文件数 + 行数增删，不区分文件级新增/修改/删除                              | `mode-task-types-core.ts` 的 `ModeTaskChangeSummary` |
| 标识呈现 | 侧栏绿点（扁平/时间线/置顶/分组已一致）+ 详情页顶部通道图标                    | `bot-binding-indicators.md`（2026-10-15 修正）       |

## 需要用户拍板的决策（这四条决定重做范围，未定前不编码）

**D1 会话作用域是否升到话题级？**（`im-bot-group-chat-alignment.md` 的原 D1，仍未定）
现状 `getBotActorConversation(chatType | providerUserId | chatId)` 没有话题/线程维度；MyAgents 的
session key 形如 `im:telegram:private:12345`（新架构 `agent:` 前缀），群内按 thread 隔离。

- 影响：G6 话题隔离 / G7 历史持久化 / G9 输入合并三项依赖它；牵动 `BotActor`、`BotContextState`、
  桌面会话映射、工作区绑定，以及 `bot-state.v3.json` 的版本迁移。
- 选「升」= 一次数据结构迁移 + 群聊能力可以真正对齐；选「不升」= 群聊永久保持"单群单会话"的简化。

**D2 是否引入 MyAgents 的 Agent / Channel 父子配置模型？**
参照侧：`ChannelConfig`（单通道）+ 父级 `AgentConfig`（providerId / model / permissionMode /
mcpEnabledServers / heartbeat / runtime…），通道用 `overrides` 覆盖父级，**空值继承**。
我们现状：一个 bot 就是单通道，配置里平铺 providerId / model / permissionMode / mcp。

- 影响：若引入，配置 schema（`bot-config.v3.json`）与设置页都要改，且要给出继承语义的迁移；
  这不是"接线"，是一次配置模型迁移。
- 选「不引入」= 保留平铺单通道模型，功能对齐但配置模型不同（需在 spec 里记为既定差异）。

**D3 接入是否改为 MyAgents 的三步向导（推翻 `im-bot-onboarding.md` 的 C2）？**
参照侧：创建向导 = ① Token / 凭据**验证** → ② 用户身份绑定（QR / 绑定码）→ ③ 启动；
详情页管 provider / model / permission 覆盖；列表项只有 start / stop。
我们现状：`ProviderSettingsCard` 卡片式详情 + `BotSummaryCard` 的 Switch，**刻意不做向导**（原 C2）。

- 用户 2026-10-15 的「全部重做」若包含 UX 层，则 C2 作废，需要按向导重画接入流程。
- 选「改向导」= 新建流程重画（含步骤状态机、可回退、错误态）；选「保持卡片式」= 只对齐字段与判据。

**D4 bot 运行时是否迁出 services（MyAgents 是 Rust 端跑 bot 实例）？**
参照侧：`src-tauri/src/im/`（commands / telegram / feishu / dingtalk / router / agent_channel）由 Rust 跑
bot 实例，IM 消息经 router 注入 sidecar 会话，桌面端可"加入"同一会话续聊。
我们现状：bot 运行时在 `packages/services/src/bots/`（TS）+ `*ChannelRuntime.ts`，桌面端**没有**"加入会话"入口。

- 影响：迁移是跨语言重写，收益（长连接稳定性、与宿主同进程）与成本都很大；
  与 [tauri-rewrite-direction.md] 的四层评估口径一致，需要单独一轮。
- 选「不迁」= 只补桌面端"加入/交接 bot 会话"的入口（行为对齐，不动运行时）。

## 产品规则（对齐面的行为清单）

下列 N1–N7 为「全部重做」的行为目标；标注的决策项未定前不实施。

- **N1 三步接入向导**（依赖 D3）：① 凭据录入并在线验证（复用既有 `IBotsService.testBot`，
  各 provider 的 `test` 本就是只读鉴权探测）→ ② 身份绑定（扫码或绑定码，按通道现有能力）→ ③ 启动。
  每步有可观察的就绪判据与错误态；返回上一步不丢已录入凭据。
- **N2 配置继承模型**（依赖 D2）：通道级覆盖父级，空值继承；未引入则记为实现差异。
- **N3 列表项开关**：列表只暴露 start / stop（对齐参照侧）；详情页承担其余字段。
- **N4 凭据外的管理面**：白名单（`allowedUsers`）、群权限、心跳、MCP 工具卡片的 UI 化。
  现状仅配置存在，UI 未暴露的项按实际需求逐项列进阶段计划，不一次全上。
- **N5 会话加入/交接**（依赖 D4）：桌面端能把一个 bot 会话"加入"到本地继续对话；
  与 [im-bot-remote-workspace-binding.md] 的绑定语义不冲突（加入 ≠ 绑定）。
- **N6 群聊**（G3/G4/G5/G8/G10 不依赖 D1；G6/G7/G9 依赖 D1）：引用原消息、成员名目录（不再显示「未知成员」）、
  @ 解析出具体成员并在回复里 @ 回人、群消息排队与准入提示、附件群消息不再被直接拒绝。
  达标定义沿用 `im-bot-group-chat-alignment.md` 的 G1–G10 表，不在此重复。
- **N7 回复面**：默认档流式（已是分段流式）、`replyMode` 四档与 provider 矩阵（已批准）、
  变更摘要补文件级新增/修改/删除（需要新的**行数→类型**数据源，属回复侧行为改动）。

## 硬约束

| #   | 约束                                                   | 判据                                                                                                        |
| --- | ------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------- |
| C1  | 通道封闭集 = 现有 4 个（weixin/wecom/feishu/dingtalk） | `BOT_PROVIDERS` 仍是 4 项；新建面板不出现第 5 个通道                                                        |
| C2  | 不读上游衍生实现、不读 MyAgents 源码                   | 允许输入清单见下节；唯一行为来源是团队分析报告 §5                                                           |
| C3  | 状态所有者不变                                         | 凭据 = `credentialService`；配置 = `bot-config.v3.json`；绑定 = `bot-state.v3.json`；三份都不新增第二写入方 |
| C4  | 非 4 通道 provider 只读兼容                            | telegram / lark / webhook / discord 不进产品面，已有配置仍可运行                                            |

## 状态所有者与不变式

沿用 [bot-state-ownership.md](./bot-state-ownership.md)，本 spec 不新增所有者。若 D1 选「升到话题级」，
`bot-state.v3.json` 的版本迁移是**唯一**允许的写入纪律变更，且必须幂等、可回退。

## 允许输入清单（clean-room）

写代码时**只允许**看：① 本 spec 与关联 spec 的接口面；② `packages/shared/src/bots.ts` 等类型定义；
③ 现有测试断言；④ 团队分析报告 `MyAgents-分析报告.md` §5（行为）；
⑤ 黑盒对拍（真机跑参照产品的观感对比）。
**禁止**：`packages/services/src/bots/` 下上游衍生文件的实现体、`D:\gas-claw\MyAgents-ref` 的任何源码。

## 分阶段计划

- **阶段 0（不依赖任何决策，可立即做）**：本 spec 定稿 + 重建丢接线的审计回填 +
  `bot-binding-indicators.md` 的实机验收补齐。**达标**：spec 获批；审计清单为空或已修完。

  **阶段 0 的审计已完成（2026-10-15）**，结论：**IM Bot 面在重建中零损失**——
  `8242a576` 对 `packages/ui/src` 的 260 条纯删除里 **0 条**是 bot 命名文件；
  `BotBindingMenu` / `BotGroupChatCard` / `BotSummaryCard` / `ProviderSettingsCard` /
  `botsUi` / `botOnboardingStages` 两棵树皆在且都被 import。
  本区域唯一的「漏挂载」是 `WorkspaceConversationMirrorStatus`（不是 bot，属镜像），已补回。
  另有两类**不要**误判为丢失的东西：
  - `BoundGroupsCard` / `useBotGroupTask` / `useBotBoundGroups` / `useBotGroupMemberNames` /
    `BotTopicPreparation`：只出现在 `.agents/skills/.../zcode-feature-graph.yaml`（上游 v3.15.1 特性图快照）
    与旧测试 fixture 里，**两棵树都没有源码**，属上游命名，勿按此名重实现；
  - 整批 `WebRemoteControl*` / `webRemoteControl*` 删除：**在 `upstream/zcode-baseline` 中存在**
    （上游衍生），重建按「只带自研改动」丢弃，连带 `isWebRemoteControl` / `compactForRemoteControl`
    透传链（旧树约 23 文件）。这属**上游特性面缺失**，与本 spec 的 bot 重做无关，
    要不要恢复是独立的产品决策（与 D4 的「运行时是否迁出 services」分开判断）。

  因此阶段 0 剩余动作只有：本 spec 获批 + `bot-binding-indicators.md` 的实机验收。

- **阶段 1（不依赖 D1/D4）**：N7 变更摘要类型 + 群聊 G3 引用原消息 / G4 成员名目录 / G5 附件群消息。
  **达标**：G3/G4/G5 按 `im-bot-group-chat-alignment.md` 的达标定义逐条给出证据（真机截图或单测）。
- **阶段 2（依赖 D1）**：G6 话题隔离 / G7 历史持久化 / G9 输入合并，含 `bot-state` 版本迁移。
- **阶段 3（依赖 D2/D3/D4）**：N1 向导 / N2 继承模型 / N4 管理面 / N5 加入会话 / D4 的运行时决策落地。

## 验收场景

1. 四通道各走一遍完整接入，步骤集合与参照产品行为一致（差异项在 spec 里逐条记录，不用假界面对齐）。
2. 群聊：同一群内 @ 机器人 → 回复里 @ 回发起人；成员昵称可显示；引用原消息时只带被引用的那一条。
3. 群消息带附件时进入队列而不是被拒；队列满有可读提示。
4. 变更摘要区分新增 / 修改 / 删除文件（不只行数）。
5. `pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed` 全绿；
   `packages/services` 与 `packages/shared` 相关测试按实际入口执行并如实报告。
6. **实机**（留待用户）：四通道真机接入 + 群聊行为对拍参照产品。

## 非目标

- 不新增第 5 个通道（C1）；不把 `telegram / lark / webhook / discord` 拉回产品面（C4）。
- 不引入 MyAgents 的插件式平台注册表（我们是内置 4 项）。
- 不引入 assistant 侧 im-mirror（`bot-outbound-target.md` 已列为非目标）。
- 不在本 spec 里改远程桌面、CUA、工作区镜像（各自独立 spec）。
- 不因为「全部重做」而删除现有已批准能力（粒度矩阵、typing 反馈、投递恢复、绑定语义）。
