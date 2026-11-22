# Spec：mode-task legacy 设置面（configOptions / setMode / setConfigOption / 日志路径）

真源：`packages/shared/src/mode-agent-model-state.ts`、
`packages/services/src/mode-agent/modeTaskServiceAdapter.ts` 的设置方法与
`resolveModeAgentCurrentLogFilePath`。Rust：`crates/host-channels/src/mode_config_options.rs`
+ mode-task 频道分发。消费方为 bots/automation（renderer 已走 v4 store）。

## configOptions 投影

`settingsToConfigOptions(snapshot.settings)`（TS `modeSessionSettingsToModeConfigOptions`）：

- **model**：`{id:"model", category:"model", type:"select", currentValue:
  formatModelPickerValue(current), options:[{value: provider/model[$level], name: label,
  description?, modelProviderId, modelProviderName: providerLabel ?? providerId,
  modelThoughtLevels?, modelDefaultThoughtLevel?}]}`。`formatModelPickerValue`：
  `providerId/modelId`，有 reasoningLevel 时追加 `$level`。
- **mode**：`{id:"mode", category:"mode", type:"select", currentValue:
  normalizeAvailableModeMode(current)（auto/未知 → build）, options: build/edit/plan/yolo
  （name/description 与 shared 一致）}`。
- **thought_level**：仅 `enabled` 时输出；`currentValue = current ?? defaultLevel(在
  available 内) ?? available[0] ?? ""`；options 为 `{value,name,description?}`。

## 方法

- `getTaskConfigOptions({taskId,workspacePath?})`：`resumeSession` 快照 → configOptions。
- `getTaskModelSelection({taskId,...})`：`readSession` → `settings.model.current` 或 null。
- `setMode({taskId,mode,...})`：v4 CAS `switchCollaborationMode`（首发 baseRevision=0，
  stale 用 revisionAtDecision 收敛，最多 4 次）→ resume 快照 → 索引回写；
  `auto`/`autoEdit` 语义：auto 走 legacy（未迁移，显式报错），autoEdit → build。
- `setConfigOption({taskId,configId,value,...})`：
  - `thought_level` → `readSession` 取当前 provider/model（未绑定报「请先选择模型，
    再设置思考档位」）→ CAS `switchModelConfig{provider,model,thought}`；
  - `mode` → CAS `switchCollaborationMode`；
  - `model` → 解析 picker `provider/model[$level]` 后走与 `setModel` 相同的
    v4 CAS 路径（ensure_provider_config → switchModelConfig → resume/索引）；
  - 其他 id → 不改运行时，仍回源快照并返回 configOptions。
  写后 resume 快照 + 索引回写，返回最新 configOptions。
- `setModel({taskId, modelSelection{providerId,modelId,options?},...})`：Rust 以
  provider config 同步 + v4 CAS `switchModelConfig`（thought=reasoningLevel）+
  resume 快照 + 索引回写实现，返回 configOptions（TS 仍用 legacy op，因 runtimeModel
  凭据解析；Rust 依赖 CLI 最终校验，差异已注明）。
- `setAutomationSessionConfig({taskId, modelSelection, thoughtLevel?, mode?})`：
  v4 CAS switchModelConfig（thought 非空时二次收敛）+ mode CAS + resume + 索引，
  返回 configOptions。
- `getTaskNativeSessionLogFile()`：`{provider:"glm", path, exists}`，
  path = `{MODE_LOG_DIR 或 home/.mode/cli/log}/mode-YYYY-MM-DD.jsonl`（本地日期）。

## 已知差距

`compactSession`/`goalSession`/`deliverSessionMessage`/`branchTaskFromPrompt` 仍属旧
facade 命令（compact/goal 的 v4 CAS revision 与旧协议不可互换）。真机需验证 v4 CAS
在旧 CLI 的 stale 重试路径与 setModel 的 provider 凭据边界。

## 验收

`cargo test -p host-channels --lib mode_config_options`（4 条：picker 格式、model/mode/
thought 投影、disabled 隐藏、日志文件名）+ mode-task 频道 requires-control 与日志路径
形状用例。
