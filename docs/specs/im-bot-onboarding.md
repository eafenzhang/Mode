# IM 机器人接入方式对齐（四通道固定）

状态：草案（2026-10-14）。用户决策：先出 spec 草案；接入方式**保持卡片式、补齐缺口**，不重构为多步向导。
实现进度：**R3-1 / R3-2 / R3-4 / R3-5 已实现**，**R3-3 判定为不做**（既定差异，见下）；
「待评审」三问已全部结案（钉钉扫码不做、凭据在线验证做、微信不补手填）。
本 spec 待用户批准后转「已批准」状态。

实现载体（2026-10-14 恢复记录）：本 spec 的四项补齐**先实现过一次，随后被上游同步提交
`d22c60f3`（用已提交的 main 当 ours 覆盖整个 `packages/` 子树）连未提交改动一起清掉**，
本文件与 `packages/ui/src/lib/botOnboardingStages.ts` 因未被覆盖而幸存，成为按图重做的依据；
`packages/ui/src/i18n/locales/fa.ts` 因被该提交顺手带走，反而完整保留了改动后的词条集
（新增 15 条、删除下面 R7 的 4 条），本次即按它把 en-US / zh-CN 对齐回来。
实现位置：`packages/ui/src/BotsDialog/ProviderSettingsCard.tsx`（`FeishuSetupPanel`、状态行、
删 Telegram 面板）、`packages/ui/src/BotsDialog.tsx`（手填保存 + 录入即验证）。
关联：[bot-reply-granularity.md](./bot-reply-granularity.md)、[im-typing-feedback.md](./im-typing-feedback.md)、
[wecom-reply-feedback.md](./wecom-reply-feedback.md)、[bot-outbound-target.md](./bot-outbound-target.md)、
[bot-outbound-delivery.md](./bot-outbound-delivery.md)、[im-bot-remote-workspace-binding.md](./im-bot-remote-workspace-binding.md)、
[bot-state-ownership.md](./bot-state-ownership.md)、[weixin-provider-self-id.md](./weixin-provider-self-id.md)。

## 背景

用户指令（2026-10-14）：「IM 机器人接入方式和回复对齐 MyAgents 项目的机器人 bot 功能；通道保持目前的 4 个。」

拆解成三条可判定的事实：

1. **「目前的 4 个」= UI 新建面板实际暴露的 4 个通道**：`packages/ui/src/botsUi.ts:17` 的
   `BOT_PROVIDERS` 为 `weixin` / `wecom` / `feishu` / `dingtalk`，四项 `implemented: true`。
   快捷入口（`packages/ui/src/WebRemoteControlDialog.tsx:18-35`）同样只列这 4 个。
   `packages/shared/src/bots.ts:24` 的 `botProviders` 还有 `telegram` / `lark` / `webhook` /
   `discord`（以及历史字面量 `astrbot`），但**不在产品面**。
2. **回复侧的对齐已在前几轮完成**，本轮不再改行为，只在「已对齐基线」一节固化其状态与引用。
3. **接入侧没有任何 spec**：`docs/specs/` 下 10 个 bot 相关 spec 全部覆盖接入**之后**的语义
   （绑定菜单、出站投递、出站目标、回复粒度、状态所有权、工作区切换/准入、远程工作区绑定、
   typing 反馈）。接入前的过程——凭据如何拿到、身份如何绑定、运行时如何启动——**无 spec 固化**，
   且四个通道的步骤集合并不一致。本 spec 补这一块。

## 参照：MyAgents 的接入形态（仅行为描述）

依据 `D:\gas-claw\MyAgents-分析报告.md` §5.3（该报告为团队已完成的黑盒行为分析）。
本节只记录**观察到的行为与步骤集合**，不引用其实现源码（见「风险与许可边界」）。

- 接入是**三步**：① 凭据 / Token 录入并**在线验证** → ② **用户身份绑定**（扫码或绑定码）→ ③ **启动**。
- 三步落在两个界面：创建向导负责 ①②③，详情页负责接入后的 provider / model / 权限覆盖；
  列表项只有 start / stop 开关。
- 平台注册表为内置项 + 插件项混合（其内置 Telegram / 钉钉；飞书改由插件提供）。

映射到我们的现有形态：`ProviderSettingsCard` 即"详情页"，`BotSummaryCard` 的 Switch 即
"列表项 start / stop"，`BindCodePanel` 即"绑定码"。**我们已具备同一组构件**，
差异集中在两点：**每通道的步骤集合不统一**、**缺少逐段的在线验证与状态呈现**。

## 硬约束（本轮边界）

| #   | 约束                                   | 判据                                       |
| --- | -------------------------------------- | ------------------------------------------ |
| C1  | 通道封闭集 = 现有 4 个，不新增也不下线 | `BOT_PROVIDERS` 仍是 4 项                  |
| C2  | 保持卡片式详情布局，不重构为多步向导   | 不新增 wizard 路由/步骤状态机              |
| C3  | 不改变回复侧任何行为                   | 粒度矩阵、typing、投递、出站目标全部不动   |
| C4  | 不新增非 4 通道 provider 的 UI 面      | telegram/lark/webhook/discord 维持只读兼容 |

## 现状（四通道接入路径，以实现为准）

四通道目前**不对称**，这是本轮要收敛的对象：

| 通道             | 凭据获取             | 是否支持手填                         | 绑定阶段            | 「已连接」判据                                    |
| ---------------- | -------------------- | ------------------------------------ | ------------------- | ------------------------------------------------- |
| 微信 weixin      | 仅扫码注册           | 否（无手填字段）                     | **无** `/bind` 阶段 | `credentialRef`（`ProviderSettingsCard.tsx:438`） |
| 企微 wecom       | 扫码注册 **或** 手填 | 是（`wecomBotId` + secret）          | 有，自动 `/bind`    | `providerUserId`                                  |
| 飞书 feishu/lark | 仅扫码注册           | **否**（手填入口已在 UI 重构中移除） | 有，自动 `/bind`    | `providerUserId`                                  |
| 钉钉 dingtalk    | 仅手填               | 是（`dingtalkClientId` + AppSecret） | 有，自动 `/bind`    | `providerUserId`                                  |

关键位置：

- 凭据写入的唯一路径是 `saveBot(patch, { credentialValue })`（`BotsDialog.tsx:755-775` 钉钉、
  `777-798` 企微），密文经 `credentialService.save`（key `bot:<botId>:credential`），
  UI 只持有 `credentialRef`。
- 扫码三通道的服务实现：`weixinRegistration.ts`、`wecomRegistration.ts`、`feishuAppRegistration.ts`；
  服务面转发在 `botsService.ts:6975-6992`。
- 自动接入状态机 `BotsDialog.tsx:1122-1275`：无凭据时自动开扫码面板；有凭据但未绑定时自动生成
  `/bind` 码；绑定码过期自动续码。该状态机已按 provider 分支，是 C2「保持卡片式」的实现基础。
- 微信的"激活"是**另一个概念**：`isConnected` 只看 `credentialRef`，真正的激活由首条入站消息
  触发并写 `weixinActivatedAt`（`botsService.ts:1987-2021`），UI 轮询后展示激活提示
  （`ProviderSettingsCard.tsx:473-481`）。
- 飞书扫码成功后的注释（`BotsDialog.tsx:553-554`）明确记载："服务层一直支持飞书扫码注册，
  但 Bots 重构后的 UI 只保留了手填凭据"——说明**手填与扫码两条路径历史上都实现过**，
  现在各通道各自只剩一条。

## 产品规则

### R1 三段式定义（本轮统一口径）

任一通道的接入都由三段构成，每段有独立、可观察的就绪判据：

```text
① 凭据就绪   credentialRef 存在（扫码或手填写入，写入路径唯一 = saveBot）
② 身份绑定   providerUserId 存在（私聊 /bind 消费，或平台自带绑定）
③ 运行启动   services reconcile 后 runtime.status ∈ {running, error}
```

`①` 与 `②` 的产物分别落在 `bot-config.v3.json` 与凭据库；`③` 不落配置，由 services 决定。

### R2 步骤集合并存而非替换

每通道**允许**的路径按现状冻结，补齐只做「加回缺失的那条」，不删除现存的任何一条：

- 企微：扫码 + 手填（现状即双路，作为模板）；
- 飞书：扫码为默认路径，**补回手填**（见 R3-1）；
- 钉钉：手填为默认路径，**扫码为可选补齐**（见 R3-3）；
- 微信：扫码为唯一路径，**不引入 `/bind` 阶段**（其绑定语义由平台在扫码时一并完成，
  见 R3-2 的显式化要求）。

### R3 本轮补齐项（需评审确认后实现）

1. **飞书补回手填 App ID / Secret 入口。**（**已实现** 2026-10-14）
   服务层无需改动：`saveBot({ ...bot, feishuAppId }, { credentialValue: appSecret })` 即走
   既有校验与加密写入。UI 在飞书无凭据分支由"只有扫码按钮"改为"扫码 + 手填"双向面板
   （`FeishuSetupPanel`，与企微 `WeComSetupPanel` 同构），绑定码仍由既有「有凭据但未绑定」
   状态机生成。
   - 词条复用而非新增：`bots.feishuAppIdPlaceholder`、`bots.feishuAppSecretPlaceholder`、
     `bots.feishuRegistrationTitle/Description/Button` 是当年手填面板被移除后的**遗留孤儿**
     （`packages/ui/src/i18n/locales/*.ts` 中存在但无引用），恢复时直接复用；仅新增
     `bots.feishuCredentialsMissing`、`bots.feishuManualTitle`、`bots.feishuManualDescription`、
     `bots.feishuSaveManual`、`bots.feishuManualSaved` 五个词条（en-US / zh-CN / fa-IR 同步）。
   - 副产物：飞书无凭据时的 `control` 槽位由「扫码按钮」改为「尚未配置凭据」文案，
     与企微分支一致；扫码按钮移入面板内（`bots.feishuRegistrationButton`）。
     原先独立的 `hasActiveFeishuRegistration` 详情分支被该面板吸收后删除，
     扫码态二维码/用户码/扫码提示的呈现不变。
2. **微信的绑定态与激活态显式化。**（**已实现** 2026-10-14）
   现状 `isConnected = credentialRef`，把"凭据已就绪"和"机器人已可用"混成一个点：
   扫码成功后圆点即变绿、文案为"已连通"，但此时机器人还收不到消息。
   按 R1 口径拆成两段呈现：凭据就绪（`credentialRef`）与已激活（`weixinActivatedAt` 存在）。
   - 落地方式：微信不设「绑定」段（扫码时平台一并完成身份绑定，`/bind` 对它不适用，
     这是 spec R2 刻意保留的不对称），第三段改为「激活」，就绪判据为 `weixinActivatedAt`；
     未激活时为灰点「待激活」，不再显示绿色的「已连通」。
   - 未改变的部分：现有激活提示文案（提示用户发首条消息）保留，不新增阻塞性步骤，
     也不添加 `/bind` 入口。
3. **钉钉扫码注册 —— 判定为「不做」（既定差异）。**（2026-10-14）
   证据：仓库里企微（`wecomRegistration.ts`）、飞书（`feishuAppRegistration.ts`）、
   微信（`weixinRegistration.ts`）都有完整的扫码注册模块，**钉钉没有任何注册实现**——
   `dingtalkChannelRuntime.ts` 只做 Stream 长连接的订阅注册（`/v1.0/gateway/connections/open`），
   不涉及机器人创建；UI 侧 `DingTalkCredentialPanel` 也只收手填凭据。
   公开文档侧未能取到可佐证的接口（DingTalk 文档站为 SPA，抓取无有效内容），
   因此按 R3-3 原文的规则记为**与参照产品的既定差异**：不做假界面、不加占位按钮。
   若确认钉钉开放平台确有等价接口，再单开一轮补 `beginDingtalkRegistration` / `pollDingtalkRegistration`。
4. **逐段状态在卡片上可见。**（**已实现** 2026-10-14）
   三段状态收成卡片上的一行（凭据 / 绑定 / 运行，微信为 凭据 / 激活），取代原来分散在
   圆点与按钮文案里的多处呈现。数据源仍是现有的 `getStatus()` 与 `getConfig()`，
   不新增状态所有者。
   - 判定收成纯函数 `packages/ui/src/lib/botOnboardingStages.ts`（`deriveBotOnboardingStages`），
     卡片只做渲染；圆点配色沿用 `shared.tsx` 既有的 `runtimeDot` 词汇
     （就绪绿 / 待办灰 / 异常红）。
   - **词条**：状态行按 `bots.onboarding.<段>.<态>` 取文案，共 9 条——
     `credentials.done|pending`、`binding.done|pending`、`activation.done|pending`、
     `runtime.done|pending|error`（en-US / zh-CN / fa-IR 三语同步）。
   - **渲染位置**：`ProviderSettingsCard` 的 `control` 槽位（原「圆点 + 一句话状态」所在的同一处）。
     `BotSummaryCard`（侧栏列表项摘要）不改——那里是列表摘要，放不下逐段状态行。
     凭据段未就绪时整行不渲染，`control` 保持原来的「尚未配置 …凭据」提示（见上「未接管的部分」）。
   - **与原文的一处刻意偏离**：原文写"取代……错误面板"，实现上**保留了**飞书运行时错误面板
     与投递失败面板。这两块不是状态冗余，而是可操作的诊断信息（错误码、错误详情、
     失败原因）；用一行状态把它替换掉会丢信息。状态行只接管"圆点 + 一句话状态"。
     如确要一并移除，请先确认诊断信息改到何处呈现。
   - 未接管的部分：无凭据分支的「尚未配置 …凭据」提示保持原样——在凭据缺失时，
     该提示本身就是第一段的状态与行动入口，再加一行状态只是同义重复。
5. **录入即验证（凭据在线验证）。**（**已实现** 2026-10-14，原「待评审」第 1 问）
   对齐参照产品向导第一步的"录入并验证"：手填凭据保存成功后立刻做一次在线校验，
   把"凭据写错要等 runtime 报错才可见"提前到保存当下。
   - **不新增服务层接口**：复用既有 `IBotsService.testBot(botId)`，各 provider 的适配器
     `test` 本就是只读鉴权探测，不投递任何消息——企微 `probeAuthentication`、
     飞书 `readTenantAccessToken`、钉钉 `getAccessToken`、微信 `requestWeixinJson("/getconfig")`、
     Telegram `getMe`。UI 侧的 `botsService` 是 `ProxyChannel.toService<IBotsService>`
     通用代理，方法按名转发，所以调用零接线。
   - **失败不阻断保存**：凭据已经落盘，此时提示为「凭据已保存，但在线验证未通过：{error}」
     并附平台原文（warning 样式）。理由：平台临时不可达不应被误判成凭据写错，
     更不该阻止用户保存。
   - **文案词条**：`bots.credentialsVerifyFailed`（en-US / zh-CN / fa-IR 三语同步）。
   - **只覆盖手填路径**（钉钉 / 企微手填 / 飞书手填）：扫码拿到的凭据由平台颁发，
     再探测一次只是多一次网络往返。扫码路径的异常继续由既有的运行时状态呈现。
   - **停用的机器人跳过验证**：`test` 在 `enabled=false` 时会直接返回"… is disabled."，
     那不是凭据问题，因此跳过（保存提示照旧）。

### R4 凭据边界不变

凭据只经 `credentialService` 存储；UI 只持 `credentialRef`；明文不落配置文件。
本轮补齐项不得引入第二条写入路径（沿用 `saveBot`）。

### R5 启动 / 停止的所有者不变

UI 只写 `enabled` 并读 `getStatus()`；运行时启停由 services 的 `reconcile` 完成
（`channelRuntime.ts` 的锁与刷新队列）。UI 不做乐观运行态。

### R6 自动接入状态机保留

`BotsDialog.tsx:1122-1275` 的自动行为（无凭据自动开扫码、有凭据未绑定自动出 `/bind`、
码过期自动续）是本轮 C2 的实现基础，**不删不改语义**；补齐项只在该状态机的分支里追加新形态。

### R7 非 4 通道 provider 只读兼容

`telegram` / `lark` / `webhook` 仍有完整服务层路径（可被已有配置驱动），`discord` / `astrbot`
不可达。新建面板维持 4 项。

**2026-10-14 追加（用户指令「不符合当前通道的去掉」）：移除 Telegram 接入面板。**
原先 `ProviderSettingsCard` 为 telegram 保留了一整套"打开 BotFather + 二维码 + token 手填"面板
（`TelegramBotFatherQrPanel`），但 telegram 不在产品暴露的 4 个通道内，新建面板也从不暴露它——
只有历史配置能走到，属残留。改动：

- 删除 `TelegramBotFatherQrPanel` 与 `telegram && !hasSecret` 控制分支；随之删除只服务于它的
  `onSaveSecret` / `onOpenTelegramBotFather` 两个 prop、`handleSaveSecret` /
  `handleOpenTelegramBotFather` 两个处理函数、`TELEGRAM_BOTFATHER_URL` 常量，以及
  `usePlatform`（该文件唯一用途是打开 BotFather 链接）。
- 清理因此失去引用的词条：`bots.openBotFather`、`bots.telegramBotFatherQrAlt`、
  `bots.telegramBotFatherScanHint`、`bots.credentialPlaceholder`（en-US / zh-CN / fa-IR 同步）。
- **服务层不动**：telegram 的适配器、运行时与配置 schema 照旧，已有 telegram 配置仍能凭既有凭据
  运行（与 lark / webhook 同属只读兼容），只是不再能通过 UI 编辑凭据。这与「通道保持 4 个」
  一致：产品面收口，数据面不破坏。
- 未采纳的候选项：钉钉 AI 卡片模板、心跳、私聊「全部用户」三项经用户确认**保留**
  （它们属于使用场景判断，用户判定为仍在场景内）。

### R7 只读兼容面清单（2026-10-16 核对确认，**不是遗漏，勿"补全"**）

产品面收口在 4 个通道，但数据面刻意保留下面这些"只读兼容"的残留。它们的存在是**刻意的**，
每一处都请在改动前先读本节：

| 面 | 现状 | 处置口径 |
| --- | --- | --- |
| `packages/shared/src/bots.ts` 的 `botProviders` | 含 `telegram` / `lark` / `webhook` / `discord`（+ 历史字面量 `astrbot`） | 服务层适配器与运行时照旧保留，存量配置继续能跑；**不进新建面板** |
| `packages/ui/src/botsUi.ts` 的 `BOT_PROVIDERS` | 只有 `weixin` / `wecom` / `feishu` / `dingtalk`，四项 `implemented: true` | **新建面板的唯一事实源**；与上面那个枚举不一致是刻意的 |
| Telegram 接入面板 | 已删（`TelegramBotFatherQrPanel`、`TELEGRAM_BOTFATHER_URL`、BotsDialog 里的 `usePlatform`） | 存量 telegram 配置**不能再经 UI 编辑凭据**，但凭既有凭据仍可运行 |
| `RETIRED_BOT_PROVIDERS` | 解析配置时丢弃（AstrBot 已彻底下线） | 与"只读兼容"不同：这些是真删除，配置里的记录会被解析阶段清理 |
| `BotGroupChatConfig.allowedGroups` | 群白名单已下线，字段仅保留以便解析 | 不恢复功能；群服务范围现在由「私聊方式 + 群聊方式」决定 |

判断某个 provider 属"只读兼容"还是"已下线"的唯一依据：是否出现在 `RETIRED_BOT_PROVIDERS` 里。
只读兼容项**不新增 UI、不新增文案**，但也不删服务层代码——删了会让存量配置的机器人直接失效。

## 已对齐基线（回复侧，本轮不改）

回复侧的"对齐 MyAgents"已分四轮落地，本 spec 只固化引用，不重新打开：

| 面         | 规则                                                                               | 来源                                                                                |
| ---------- | ---------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------- |
| 回复粒度   | 四档 + provider 矩阵；企微默认 `streaming_card`，其余 `assistant_changes`          | `bot-reply-granularity.md`（commit `67fe0110`）                                     |
| 等待期反馈 | 微信 typing 票 / 飞书 reaction / 企微静态占位流 `WECOM_TYPING_PLACEHOLDER`         | `im-typing-feedback.md`、`wecom-reply-feedback.md`（commit `495369e9`、`ae05f1c9`） |
| 出站目标   | 群回群、私聊回本人；`resolveOutboundTarget(actor) = chatId ?? providerUserId` 唯一 | `bot-outbound-target.md`（commit `c9bac3be`）                                       |
| 投递可靠性 | 失败分类、停放补投、微信 context 过期强刷票重试                                    | `bot-outbound-delivery.md`（commit `12444686`）                                     |

## 状态所有者与时序

接入过程只写**已有**的两个所有者，不新增第三个：

```text
用户操作（扫码 / 手填 / /bind）
   │
   ├─ 凭据 ──▶ credentialService（bot:<botId>:credential）
   │
   ├─ 配置字段 ──▶ BotsRepo.writeConfig ──▶ bot-config.v3.json
   │                （feishuAppId / wecomBotId / dingtalkClientId / providerUserId）
   │
   └─ enabled ──▶ BotsRepo.writeConfig ──▶ services reconcile ──▶ runtime.start/stop
                                                                     │
UI 只读 getStatus() / getConfig() ◀───────────────────────────────────┘
```

单所有者声明：**凭据唯一所有者 = `credentialService`；配置唯一所有者 = `bot-config.v3.json`；
运行态唯一所有者 = services 的 reconcile**。UI 是纯视图，不持有第二份接入状态。
（`bot-state.v3.json` 承载对话/通道游标，接入过程不写它，微信 `weixinActivatedAt` 除外——
它由入站消息写入，属既有的通道级状态。）

## 接口

- **不新增服务层接口。** 补齐项全部复用 `saveBot` / `getStatus` / `createBindCode`。
- UI 侧拟新增（均为组件内，不外泄）：
  - 飞书手填面板（复用企微 `WeComSetupPanel` 的字段形态）；
  - 三段状态行的派生函数（由 `getConfig()` + `getStatus()` 纯函数推导）。
- 若评审决定做钉钉扫码，才需新增 `beginDingtalkRegistration` / `pollDingtalkRegistration`
  与其类型——**仅在平台确有该接口时**。

## 验收场景

1. **通道封闭**：`BOT_PROVIDERS` 仍为 4 项；新建面板渲染 4 个按钮；无新增 provider 条目。
2. **飞书双路**：无凭据时扫码与手填两个入口同时可用；手填保存后 `credentialRef` 与
   `feishuAppId` 同时落盘，且随后自动进入 `/bind` 分支（复用既有状态机，不新增路径）。
   - 自动化现状：`packages/ui` **没有测试运行器**（`packages/ui/package.json` 只有 lint；CI 的
     verify 只跑 shared / services / server / core / desktop），因此本项暂以
     `pnpm typecheck` + `pnpm lint` + 人工实机为准，不新造一个不会被执行的测试文件。
     若后续给 `packages/ui` 接入 `node --test` 运行器，本项应补为对
     `FeishuSetupPanel` 的渲染契约测试。
3. **微信两态**：仅有 `credentialRef` 时显示"凭据就绪、未激活"；收到首条入站消息写入
   `weixinActivatedAt` 后转为"已激活"。不出现 `/bind` 入口。
4. **三段状态一致性**：卡片状态行与 `getStatus()` 输出逐项一致（同一 bot 在扫码前 / 扫码后 /
   绑定后 / 运行异常四个时刻的呈现）。
5. **回归**：`pnpm --filter @mode/services test`、`pnpm typecheck`、`pnpm lint`、
   `pnpm architecture:check --changed` 全绿。
6. **实机**（留待用户）：四通道各走一遍接入（微信扫码、企微扫码+手填、飞书扫码+手填、
   钉钉手填），确认绑定与运行态呈现与桌面端会话一致。

## 非目标

- 不重构为多步向导（C2）；不引入 MyAgents 的 Agent/Channel 父子配置模型
  （我们的 bot 就是单通道，见 C1）。
- 不改回复侧任何行为（C3），包括粒度的 provider 默认值、typing 机制、投递恢复顺序。
- 不让 `telegram` / `lark` / `webhook` / `discord` 进入新建面板（C4）。
- 不做凭据的在线"验证"独立步骤（见「待评审」第 2 条）。
- 不改 `bot-state.v3.json` 的写入纪律（`bot-state-ownership.md` 已固化）。

## 待评审（已全部结案，2026-10-14）

1. ~~**钉钉扫码**~~ —— 已结案：判定为不做（既定差异），理由见 R3-3。
2. ~~**是否引入独立的"凭据在线验证"步骤**~~ —— 已结案：**做**，见 R3-5。
   原问题里"要新增验证调用、会触碰不新增服务层接口"的顾虑不成立：服务层早在
   `IBotsService.testBot` 上就有这个能力，适配器 `test` 也一直是只读鉴权探测，
   本轮只是把 UI 接上去。
3. ~~**微信是否补手填凭据**~~ —— 已结案：**不补**（用户决策 2026-10-14）。微信维持扫码唯一路径。
   影响：微信接入依赖扫码链路本身可用，没有手填兜底；这是刻意接受的取舍，
   不为此添加半成品入口。若平台日后开放手工填入 bot id/secret，再按 R2 的"双路"补齐。

## 风险与许可边界

- **参照产品许可**：MyAgents 为 AGPL-3.0。本 spec 只引用团队已完成的**行为分析**结论
  （报告 §5.3）描述步骤集合，不引用其实现源码；实现时同样只对齐**行为与步骤**，
  不翻译、不复制其代码。若后续需要引入其任何第三方组件，走 `third-party/` 台账纪律。
- **不对称收敛风险**：R2 的"并存而非替换"会让四通道的步骤集合仍然不同（微信无 `/bind`、
  钉钉无扫码）。这是刻意的：以平台实际能力为准，不用假界面凑一致性。
- **状态呈现重构风险**：R3-4 会改动卡片的既有呈现（圆点 / 错误面板），需回归
  `bot-binding-menu-display.md` 与 `wecom-reply-feedback.md` 中依赖这些呈现的既有验收项。
