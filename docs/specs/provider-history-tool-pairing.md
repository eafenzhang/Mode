# Spec：Provider 请求历史的 tool_use / tool_result 配对自愈

## 背景

线上曾出现一次硬失败（会话数据不在本机，仅存错误文本）：

```text
unexpected `messages.823.content.0: tool_use_id` found in `tool_result` blocks:
call_00_gwRWMn1UQ4eeJpdSNxc79094. Each `tool_result` block must have a
corresponding `tool_use` block in the previous message.

Turn execution failed
provider=deepseek-2 provider_code=invalid_request_error model=deepseek-flash
request=4ed05def-… reason=invalid_request status=400 retryable=false
```

即：发给 provider 的 `messages[823]` 以 `tool_result` 开头，但 wire 分组后的
「前一条消息」里没有与之配对的 `tool_use`。Anthropic 风格端点（deepseek 的
`/anthropic` 兼容端点按同一套 schema 校验）对该形态返回 400 且 `retryable=false`，
整个 turn 直接失败。

### 调查结论（根因现状）

- 对当前检出源码做了全链路审计：历史组装（`buildProviderRequestMessages` 的
  reorder / MCS / legacy-reminder 投影）、适配转换（`toAiSdkMessages`）、
  Anthropic 分组（`groupIntoBlocks`）、冷恢复 hydration、compact 分组切片、
  session fork 重映射——**现有写入路径在结构上均保持配对**。
- 历史已出现过两次同族缺陷并分别修复（见 `turn-model-step.ts` 与
  `streaming-tool-coordinator.ts` 中「合并修复」注释），说明**破坏配对的写入方
  会随功能演进反复出现**，而非一次性问题。
- 失败请求的原始 payload（messages 822/823）在本机两个数据根
  （`~/.mode`、`~/.zcodium`）均不存在，无法定位到具体的单个写入方。
- 修复前，全仓库**没有任何**针对该配对不变量的校验或自愈。

因此本 spec 定义的是**请求边界的不变量与自愈语义**：无论上游哪个写入方再次
破坏配对，发往 provider 的请求都保证符合语法；自愈时的取证日志用于下一次
定位真实写入方。

## 不变量

对每次物理请求的 `messages`（`ModelInputMessage[]`，适配转换之前）：

- 按 wire 分组规则（相邻 `user`/`tool` 合并成一个 user block；`system`、
  `assistant` 各自成块），每个 `role === "tool"` 的 `toolCallId` 必须被
  **分组后的前一条消息**声明：即从该条目向前越过相邻的 `user`/`tool` 后遇到的
  第一条消息必须是 `assistant`，且其 `toolCalls` 含同一 `id`。
- 等价判据（实现所用）：从 `tool` 条目向前扫描，遇到 `assistant` 或 `system`
  停止；停在 `assistant` 且含该 `id` → 配对成立；否则为孤儿
  （孤儿含三种成因：声明方缺失、`system` 隔断、`tool_result` 重复）。

## 所有权与修复边界

```text
core（历史写入方，不变）
   └─ 唯一 canonical history owner；本 spec 不改任何写入路径
adapters/model（本修复的唯一 owner）
   └─ runner-generate / runner-stream 在 provider 拒绝后，
      仅改写「本次请求副本」（request-local），一次性重试
   └─ 永不回写 canonical history
```

- 触发条件（两个条件同时成立才动手，与 thinking 签名修复同口径）：
  1. `unwrapRetryError` 后状态码为 400；
  2. 错误文案匹配 Anthropic 配对语法错误：包含 `tool_result` 与 `tool_use`，
     且包含 `corresponding` 或 `previous message`。
- 匹配成功但检测不到孤儿 → 不重试（修复是 no-op 时不消费重试机会）。
- 修复动作（对请求副本，按原始顺序处理每个孤儿 `tool_result`）：
  1. **重新锚定**：其 `id` 在更早的某条 `assistant` 中存在声明
     （中间被 `system` 或其它消息隔断）→ 把该 `tool` 条目移动到声明方
     紧后方（声明方已有的连续 `tool` 结果之后），信息不丢失；
  2. **丢弃**：更早的声明不存在（声明方整段缺失）→ 丢弃该 `tool` 条目，
     并 warn 记录；
  3. **丢弃重复**：该 `id` 已有配对成立的 `tool_result` → 丢弃后来的重复条目。
- 重试：给一次**不占普通 retry 预算**的物理请求（与
  `repairReasoningHistoryAfterSignatureRejection` 同机制）；thinking 签名修复
  与本修复各自独立计一次，互不挤占对方。
- 时机守卫（stream 路径）：仅在 `!emittedRetryBoundaryEvent`（尚无可视输出）
  时允许修复重试，与既有签名修复一致。
- 取证日志：`warn` 级，记录 `reanchoredToolCallIds` / `droppedToolCallIds`
  与事件名 `model.tool_pairing_repair.retry`，供后续定位真实写入方；
  不打印消息正文（避免泄漏用户内容）。

## 非目标

- 不在健康请求上做主动改写（不引入第二套历史语义；自愈只在 provider 已拒绝后
  生效，保证零误伤）。
- 不修复尚未定位的历史写入方（本机无失败 payload；靠取证日志等下一次现场）。
- 不处理 thinking 签名类 400（既有 `repairReasoningHistoryAfterSignatureRejection`
  行为原样保留）。
- 不改变 `role === "user"` 且带 `toolCallId` 的旧式条目（适配层将其渲染为纯
  文本 user，不参与 wire 配对校验）。

## 验收场景

1. 边界合法配对（相邻 user 穿插、多个 tool 结果共享一个 assistant）→ 原样返回，
   不触发重试。
2. 声明方 assistant 缺失（孤儿 `tool_result`）→ 丢弃，返回修复副本 + id 列表。
3. `system` 消息隔断在声明方与结果之间 → 重新锚定到声明方之后。
4. 同一 `id` 出现第二次 `tool_result` → 丢弃重复者。
5. 非 400、或 400 但文案不匹配 → `undefined`（不重试）。
6. 匹配文案但无孤儿 → `undefined`（no-op 不消耗重试）。
7. generate 与 stream 两条路径均接入；stream 在已有可视输出后不修复。
8. `pnpm typecheck`、`pnpm lint`、目标包 `test` 全部通过。
