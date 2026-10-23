# Bot 访问控制统一双模式（whitelist / free + 黑名单 + 自动登记）

状态：草案（2026-10-23，设计已口头确认，待 spec 审阅）。
关联：[im-bot-agent-channel-aplus.md](./im-bot-agent-channel-aplus.md)、[bot-weixin-open-dm.md](./bot-weixin-open-dm.md)、
[im-bot-group-chat-alignment.md](./im-bot-group-chat-alignment.md)、[im-bot-onboarding.md](./im-bot-onboarding.md)。

## 背景

对齐 MyAgents 0.5.5（只对行为，clean-room 不读实现）：

- CHANGELOG「聊天机器人访问控制」：支持白名单模式与自由模式，可按用户或群聊拉黑、解除拉黑；
  自由模式下自动登记实际使用的用户与群聊。
- CHANGELOG「渠道权限设置」：统一白名单默认行为，重新整理用户绑定与群管理入口。

现状差距：我们已有 `privateChatMode: bound_users | all_users`（仅私聊维度）与群
pending→UI 批准流程，缺**黑名单**（两种模式下可否决）、**自由模式自动登记**
（用户与群的可见清单）与**统一的白名单缺省**。

用户决策（2026-10-23）：

1. 改成统一双模式（accessMode 统管私聊与群聊），不保留「只加黑名单」的最小方案；
2. 两个功能（访问控制 + 跨聊天交接）都做，本 spec 只覆盖访问控制；
3. **全渠道缺省 whitelist**——推翻 [bot-weixin-open-dm.md](./bot-weixin-open-dm.md) R1 的
   微信缺省 `all_users`（存量不锁死，见 R5）；
4. free 模式下群免批、自动登记。

## 产品规则

### R1 accessMode 统一准入模式

`BotConfig` 新增字段 `accessMode: "whitelist" | "free"`，语义统管私聊与群聊：

| 模式      | 私聊                                     | 群聊                                        |
| --------- | ---------------------------------------- | ------------------------------------------- |
| whitelist | 绑定用户 / `allowedUsers`（现 bound_users 行为） | pending→UI 批准（现行为），批准后全员可触发 |
| free      | 任何用户放行，首个消息自动登记           | 群免批直接可用，首个消息自动登记            |

`privateChatMode` 停止写入；纯函数 `resolveBotAccessMode(bot)` 是唯一解析入口，
调用方（准入、主动投递、UI）不得各自重写缺省。映射：`bound_users→whitelist`、
`all_users→free`。群激活模式 `disabled/mention/always` 保持独立开关，不并入 accessMode。

### R2 黑名单是否决层

- `blockedUsers: string[]` 与 `blockedGroups: { groupId, groupName?, platform, blockedAt }[]`
  挂在 BotConfig；解除拉黑即从数组移除。
- 检查顺序：**黑名单最先**，两种模式下都生效，命中即静默拒绝——不回复、不登记、
  不写群历史、不建对话。
- 黑名单只减不加：它是对授权结果的否决，不是第二条授权路径，因此不违背
  im-bot-group-chat-alignment.md R3「唯一准入开关」的精神（R3 措辞同步修订，见修订节）。
- 同一对象既在群批准名单又在黑名单时黑名单优先；free 模式下被拉黑的用户/群
  不会被自动登记复活。

### R3 free 模式自动登记

- 私聊：free 模式下首个入站消息把 `providerUserId` 追加进
  `userRegistry: { providerUserId, displayName?, discoveredAt }[]`（平台提供昵称时一并记录）。
- 群聊：free 模式下首个触发消息把群写入现有 `groupChat.permissions[]`，
  `status: "approved"`、`addedBy: "auto"`（复用现有 schema，不新增列表）。
- 登记的目的就是可见与可管理：UI 能看到谁在用，可一键拉黑或移除；
  移除登记不等于拉黑，移除后该对象回到「未登记」状态（free 下条消息会重新登记，
  要断根用拉黑）。

### R4 whitelist 模式行为不变

私聊绑定/`allowedUsers`/`/bind`、群 pending→批准、发送者准入
（群已批准或发送者在 `allowedUsers`）全部保持，不改 A+ spec 的判定分支，
只把判断入口从 privateChatMode 换成 accessMode。

### R5 缺省与迁移（不锁死存量）

- **新建渠道**：`createDraftBot` 一律显式落盘 `accessMode: "whitelist"`（含微信）。
  这推翻 bot-weixin-open-dm 的微信缺省 free——新扫码的微信 bot 默认拒私聊，
  用户切 free 或 `/bind` 后可用。
- **存量配置**：`normalizeBotConfig` 读取时按以下顺序推导并补齐 `accessMode`，
  随下次写盘持久化；**不静默改变已放行用户的可达性**：
  1. 显式 `accessMode` 优先；
  2. 显式 `privateChatMode`：`bound_users→whitelist`、`all_users→free`；
  3. 都缺省：微信→`free`（沿用 bot-weixin-open-dm R1 的旧缺省）、其余→`whitelist`。
- 配置文件保持 v3、不升版本；`privateChatMode` 字段保留一版供回滚解析，不再写入。
- 主动投递目标 `resolveBotProactiveUserId` 把 `free` 等价于旧 `all_users` 处理，
  优先级链不变（weixin-open-dm R4）。

### R6 UI：新增访问模式卡，黑名单集中管理

- 新增 `BotAccessModeCard`：访问模式下拉（白名单/自由）+ 黑名单区
  （blockedUsers / blockedGroups 两个列表，解除拉黑按钮）。
- `BotPrivateChatCard` 移除「私聊方式」下拉（移到访问模式卡）；`free` 下展示
  `userRegistry` 列表（移除、拉黑），`whitelist` 下保留已绑定用户 + 绑定码入口。
- `BotGroupChatCard`：`whitelist` 下显示现有待审批列表；`free` 下改为显示
  自动登记群（`addedBy === "auto"` 标记，可拉黑、可移除），不再出现审批按钮；
  激活模式下拉两模式都保留。
- 微信不渲染群卡与群黑名单段（群能力表不变）。
- 不新增 `if provider === "weixin"` 的准入展示分支；差异只来自缺省值与能力表。

### R7 同步修订既有 spec（实现时同批改文案）

| 文件                             | 条款                                             | 修订为                                       |
| -------------------------------- | ------------------------------------------------ | -------------------------------------------- |
| im-bot-group-chat-alignment.md   | R3「私聊方式+群聊激活是唯一准入开关」            | 改为「accessMode+群激活是唯一授权开关；黑名单为纯否决层」 |
| im-bot-agent-channel-aplus.md    | 私聊规则 2 缺省「微信 all_users，其余 bound_users」 | 缺省统一 whitelist；旧缺省仅作存量推导         |
| bot-weixin-open-dm.md            | R1 缺省表（微信 all_users）                      | 新建渠道缺省 whitelist；R1 降级为存量推导规则  |
| im-bot-onboarding.md             | 引用微信缺省 all_users 的段落                    | 引用改为 bot-access-mode R5                   |

## 状态所有者

```text
createDraftBot（UI）──► bot-config.v3.json: accessMode（新建显式 whitelist）
存量无字段 ──────────► normalizeBotConfig 推导补齐（R5 顺序）
                              │
              ┌───────────────┼────────────────────┐
              ▼               ▼                    ▼
   withAuthorizedContext   resolveBotProactiveUserId   BotAccessModeCard /
   （R2 黑名单 → R1 模式）  （free ≡ 旧 all_users）      BotPrivateChatCard /
                                                        BotGroupChatCard（R6）
```

- 唯一写入方仍是 `repo.writeConfig` / `mutateConfig`；黑名单、userRegistry、
  群 `addedBy:"auto"` 记录都随 BotConfig 同文件原子写。
- 黑名单检查在 `withAuthorizedContext` 最前，先于去重后的命令解析分支；
  被拒消息不进入 `groupHistory.append`。

## 验收场景

1. `packages/services/test/bot-access-mode.test.ts`（新增）：
   - 新建四通道 bot 均落盘 `accessMode: "whitelist"`；微信也不例外；
   - 存量微信无字段 → free 放行并登记；存量微信显式 `bound_users` → whitelist 拒绝；
   - free 私聊首条消息进 `userRegistry`，第二条不重复登记；
   - free 群首条触发消息写 `permissions[]` 且 `status:"approved" + addedBy:"auto"`，
     无审批步骤直接可触发；
   - whitelist 下群仍走 pending→批准，未批准只记历史；
   - blockedUsers 命中：whitelist/free 都静默拒绝，不登记、不写历史；
   - blockedGroups 命中已批准群：拒绝优先，`/help`、`/send` 例外是否放行按
     A+ 私聊规则 8 的例外表（黑名单不豁免例外表，即例外命令同样被拒——
     黑名单是硬否决）；
   - 解除拉黑后恢复原模式行为。
2. `packages/ui` 契约测试：`BotAccessModeCard` 调 `resolveBotAccessMode`；
   新建 bot 显式写 whitelist；free 下群卡无审批按钮。
3. 实机：新建微信 bot 私聊被拒（缺省收紧可见）；切自由模式后任意用户可用且
   出现在登记列表；拉黑后同一用户消息无响应、登记列表可解除拉黑。
4. `pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed`、
   `pnpm --filter @mode/services test` 通过。

## 非目标

- 不删除 `/bind`、绑定码、`allowedUsers`、群批准流程（whitelist 模式仍用）；
- 不做跨 bot / 平台级封禁；黑名单只在单 bot 配置内生效；
- 不把黑名单当授权路径（纯否决，不出现「拉黑白名单」式反转）；
- 不改群激活模式（disabled/mention/always）语义；
- 不做 MyAgents「重新整理用户绑定与群管理入口」的入口重排——入口只按 R6 增改；
- 不做 v4 配置迁移（A+ 阶段 2 的独立工作）。
