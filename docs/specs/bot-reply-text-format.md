# Bot 出站文本的通道渲染差异（纯文本通道的 markdown 降级）

状态：草案（2026-10-16，用户指令「主力是企微和微信，尤其对齐这两个通道」）。
关联：[bot-reply-granularity.md](./bot-reply-granularity.md)（气泡档与逐块刷出）、
[bot-outbound-target.md](./bot-outbound-target.md)（出站目标唯一函数；该文非目标 3 提到的
「渲染通道差异」由本 spec 收口）、[weixin-provider-self-id.md](./weixin-provider-self-id.md)
（微信出站协议与 `from_user_id`）。

## 背景

业务层的 bot 文案**默认通道能渲染 markdown**：`packages/services/src/bots/messages.ts`
成体系地使用粗体（`helpHelp: "**/帮助** — ..."`，连微信专属的
`weixinActivatedWelcome: "微信 Bot 已激活。发送 **/帮助** 查看命令..."` 也是），
`replyFormatter.ts` 用行内代码包路径与命令（`` `src/a.ts` ``），
流式卡片渲染器用 `####` 标题与 `**工具摘要**`。

但**通道的渲染能力并不一致**（逐条对着 provider 的出站协议核实）：

| 通道 | 出站形态 | markdown |
| --- | --- | --- |
| 企微 wecom | `msgtype: "markdown"` / `replyStream` 卡片 | 渲染 |
| 飞书 feishu | Card JSON 2.0 | 渲染 |
| 钉钉 dingtalk | AI 卡片 / Markdown 消息 | 渲染 |
| Telegram | `sendMessage` + `editMessageText` | 渲染（行内代码另行转义） |
| **微信 weixin** | `item_list[].text_item`（`type: 1`）**纯文本** | **不渲染** |

后果：微信用户看到的是一条条**裸标记**——`**/帮助**`、`` `src/a.ts` ``、`#### 标题`。
微信是主力通道之一，这是实打实的可见瑕疵（[bot-outbound-target.md](./bot-outbound-target.md)
非目标 3 曾把「渲染通道差异」留待实机确认，本轮确认：确实有诉求）。

## 产品规则

### R1 降级发生在出站边界，业务文案不分叉

markdown 降级**只在 provider 的发送路径**做（微信为 `providers/weixinProvider.ts`
的 `buildWeixinText`），业务层（`messages.ts` / `replyFormatter.ts` / 流式卡片渲染）
继续按 markdown 书写。理由：文案是共享资产，按通道分叉会立刻产生两套词条；
且新通道接入时只需声明"我支不支持 markdown"。

### R2 只剥离**我们自己产生**的标记

纯文本通道的出站文本按以下顺序处理（纯函数，`packages/services/src/bots/botText.ts`）：

1. 代码围栏行（`` ``` ``/`` ```lang ``）整行去掉，围栏**内**的内容原样保留；
2. 行内代码 `` `x` `` → `x`；
3. 粗体 `**x**` / `__x__` → `x`；
4. 行首标题标记（`#`–`######` + 空白）去掉，标题文字保留；
5. 链接 `[文字](url)` → `文字 (url)`。

**明确不处理**（避免误伤真实内容）：

- 单个 `*` 或 `_` 的斜体语法——回复里经常出现 glob（`src/*.ts`）与文件名
  （`my_file_name.md`），按斜体配对剥离会把它们吃掉；
- `- `/`* ` 行首列表符——纯文本里读起来没有歧义，保留即是最佳呈现；
- 表情、制表、以及业务层已有的纯文本（`/status` 的状态行、编号选项等）。

降级必须是**幂等**的：已降级文本再降级不变（`**` 已被剥离后不再匹配）。

### R3 不改动渲染 markdown 的通道

企微 / 飞书 / 钉钉 / Telegram 的出站文本、分片（`splitWeComText`、
`splitLongReplyText`、卡片渲染）一律不动；本 spec 只处理"纯文本通道不该收到 markdown"。

### R4 与其他出站规则的关系

- 出站目标仍只由 `resolveOutboundTarget(actor)` 决定（`bot-outbound-target.md`）；
- 逐块刷出、队列提示、命令回执、桌面镜像等**所有**走该 provider 的出站文本都经同一降级，
  不区分来源（一处生效，无第二份路径）；
- 微信既有的 CRLF 归一化（`buildWeixinText`）保持在降级之后。

## 状态所有者与时序

无新状态。

```text
业务层文案（markdown）──▶ createOutbound(actor)
                            │
                            └─▶ per-bot FIFO 发送队列
                                  │
                                  └─▶ provider.send
                                        ├─ wecom/feishu/dingtalk/telegram: 原样 / 卡片
                                        └─ weixin: toPlainTextReply(...) ──▶ CRLF 归一化 ──▶ /sendmessage
```

## 接口

- 新增纯函数（`packages/services/src/bots/botText.ts`，自研文件）：
  `toPlainTextReply(text: string): string`；
- 唯一接线点：`weixinProvider.ts` 的 `buildWeixinText`。不新增服务层接口、不改协议类型。

## 验收场景

1. 单测（`bot-reply-text-format.test.ts`）：
   - `**/帮助**` → `/帮助`；`` `src/a.ts` `` → `src/a.ts`；`#### 标题` → `标题`；
     代码围栏去掉但内容保留；`[文字](https://x)` → `文字 (https://x)`；
   - **不误伤**：`src/*.ts`、`my_file_name.md`、`- 列表项` 原样保留；
   - 幂等：重复调用结果不变。
2. 源码契约：`buildWeixinText` 必须调用 `toPlainTextReply`，且 CRLF 归一化在其后。
3. 回归：企微/飞书/钉钉的既有渲染测试（`bot-streaming-edit.test.ts`、
   `bot-wecom-typing.test.ts`）不变；`pnpm --filter @mode/services test`、`pnpm typecheck`、
   `pnpm lint`、`pnpm architecture:check --changed` 全绿。
4. **待实机**：微信侧发 `/帮助` 与一条带工具调用的回复，确认看不到 `**`、反引号与 `####`；
   企微侧同一轮回复仍保持 markdown 粗体渲染。

## 非目标

- 不做 markdown → 富文本的**升级**转换（微信没有可用的富文本载体）；
- 不改企微 / 飞书 / 钉钉的渲染与分片；
- 不按通道分叉业务词条（R1）；
- 不处理表格、脚注等我们目前不产出的语法（R2 的清单只覆盖实际会发出的标记）。
