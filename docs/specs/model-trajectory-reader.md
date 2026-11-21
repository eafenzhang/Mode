# Spec：model-io 轨迹读取（getModelTrajectory）

真源：`packages/services/src/mode-agent/modelTrajectory.ts` +
`modelTrajectoryFileTail.ts`。Rust 实现：`crates/host-channels/src/model_trajectory.rs`，
由 `mode-task` 频道的 `getModelTrajectory({taskId, limit?})` 调用。

## 数据来源

- Agent runner 落盘 `{home}/.mode/cli/{debug,rollout}/model-io-<sanitized(taskId)>.jsonl`
  （每条一行 JSON，`type: "model_io"`）；`MODE_BASE_DIR` 设置时同时尝试
  `{MODE_BASE_DIR}/.mode/cli/{debug,rollout}`。
- 文件名 sanitize：非 `[A-Za-z0-9_-]` 连续段折叠为 `-`，去首尾 `-`，截断 80；
  空结果用 `no-session`。只读该 session 单文件，按 `sessionId === taskId` 精确匹配。
- 从文件尾部读最多 32 MiB；从行中间开始时丢弃首行残片（`truncated: true`）。

## 归一化

- **delta 展开**：`messagesKind/messageOffset`（及 `sdkMessages*`、`bodyMessages*`）
  为 `"delta"` 时，以上一条已展开记录的集合前缀 `[..offset]` 拼接本条 delta；
  `"tail"` 是新起点，不拼接更早历史；缺字段/非数组/offset 非整数时不动。
- **排序**：`startedAt` 升序（`Date.parse` 等价，解析失败按 0），同值按 `requestId`。
- **记录投影**：requestId/attempt(默认 1)/startedAt/completedAt/durationMs/turnId/
  traceId/callSource/model/request{messages,toolNames}/response/error；
  可选字段缺失不输出。`callSource` 分类：main_turn→main、subagent、compact
  （`querySource=compact` 或 model.role=compact）、其余有 querySource→sidecar、
  model.role=subagent、否则 main；`session_title` 系统提示词启发式归 sidecar。
- **消息内容**：string → text（tool 角色 → tool-result，`isError` 还原
  `{type:"error-text",value}`，否则尝试 JSON.parse）；数组按 part.type 映射
  text/reasoning/tool-call/tool-result/image(file)/unknown。
- **响应**：finishReason/text/reasoningText/toolCalls（id/name/input 归一）/
  usage（input/output/total/cacheRead/reasoning）/responseId/modelId。
- **截断**：默认 200 条，`limit` 有限正数时取整；超限保留最近 N 条并置
  `truncated: true`（含文件尾部截断）。

## 失败语义

- 目录不存在/不可读、文件读失败：跳过该目录，`available` 仍为 true、records 为空。
- 单行 JSON 坏数据跳过；无匹配记录时 `sourceFiles: []`。

## 验收场景

1. `ModelTrajectoryPane` 打开某任务：显示按时间排序的调用记录、工具调用与错误状态。
2. 长 session：只返回最近 200 条且 `truncated: true`。
3. 跨 debug/rollout 两个目录：命中文件都计入 `sourceFiles`。
