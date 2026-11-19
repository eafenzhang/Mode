# CLI 评测与轨迹记录（LT-4）

> 状态：v1 首片实现。上游能力清单见 `docs/plans/2026-11-16-upstream-capability-long-tasks.md`
> 的 LT-4；本文定义首片的规则、数据形状与验收，后续扩展在 §9 追加。

## 1. 目标与非目标

**目标**

1. 给 `mode` CLI 增加 `eval` 命令：批量跑 Agent 会话用例、产出可比较的报告与轨迹。
2. 用例的输入/期望格式与现有 oracle 夹具（`packages/desktop/tests/agent-runtime-oracle/fixtures/*.json`）
   兼容：同一份 fixture 既能被桌面测试回放，也能被 `mode eval run` 执行。
3. 脚本 provider 路径完全确定：同一 suite 重复运行，报告与轨迹去掉时间字段后逐字节一致。
4. 轨迹默认脱敏，且能回放为 LT-1 的 oracle 夹具。
5. 默认不联网、不进默认门禁；真实模型评测只能显式开启。

**非目标**

- 不新建第二套 Agent 运行时：评测执行者仍是 `agent-server`（当前为 Rust 二进制，
  通过 `MODE_AGENT_SERVER_SCRIPTED_PROVIDER` 走脚本 provider）。
- 不做在线评测平台、不上传报告、不把结果当遥测。
- 不在默认 CI/`verify:pre-push` 跑真实网络或真实模型。
- 不复制桌面 oracle 的比较器实现；本包只实现 LT-4 需要的 v1 规范化规则（§6），
  由一致性测试钉住占位符词表。

## 2. 状态所有者与接口

```text
mode eval（suite/报告/轨迹所有者）
   │ 每个 case：写脚本夹具 → 起 agent-server → create/send → 收事件
   ▼
agent-server（执行所有者；脚本 provider 只读 case.script）
   │ session/event
   ▼
eval runner（投影/规范化/比较，只读）
   │
   ├──> trajectory.jsonl（每个 case 一份，单一写者）
   └──> report.json（只读汇总）
```

不变量：

- 评测进程不持有会话事实；`session/store` 仍由 `agent-server` 独占。
- 每个 case 使用独立隔离根（`MODE_DATA_BASE_DIR`/`HOME`/`USERPROFILE`）与独立 workspace，
  case 之间不共享进程、会话或数据库。
- 轨迹与报告只由本次 run 写入；回放只读。
- 所有对外产物先脱敏再落盘（§7）。

## 3. 命令

```text
mode eval run <suite.json|dir> [--eval-out <dir>] [--eval-timeout <ms>] [--eval-agent-server <path>]
mode eval replay <trajectory.jsonl> [--eval-out <dir>] [--eval-agent-server <path>]
```

- `<suite.json>`：`{ "schemaVersion": 1, "name"?: string, "cases": Case[] }`。
- `<dir>`：目录下每个 `*.json` 视为一个 `Case`（与 oracle fixtures 目录同形）。
- `--eval-out`：产物目录，默认 `<cwd>/.mode-eval`。
- `--eval-timeout`：单个 case 的总超时（默认 120000ms），超时记 `failed`，不重试。
- `--eval-agent-server`：agent-server 可执行文件。解析优先级：
  1. `--eval-agent-server`
  2. `MODE_EVAL_AGENT_SERVER_COMMAND`
  3. `MODE_AGENT_SERVER_COMMAND`（args 取 `MODE_AGENT_SERVER_ARGS_JSON`，缺省 `["app-server","--stdio"]`）
  4. 从 `cwd` 向上找 `target/{debug,release}/agent-server(.exe)`
  5. 找不到 → 退出码 1，错误码 `MODE_EVAL_AGENT_SERVER_UNAVAILABLE`，不静默跳过用例。

`--json`（全局）把报告以 JSON 打到 stdout；否则打印文本摘要。

退出码：全部通过/已录制 = 0；任一 failed = 1；配置/加载错误 = 1。

## 4. Case（与 oracle 夹具同形）

```jsonc
{
  "case": "read-single",              // 必填，suite 内唯一
  "prompt": "用 Read 读 hello.txt",    // 必填
  "seed": { "hello.txt": "hello\n" }, // 可选：运行前写入 workspace 的文件
  "script": [                          // 必填：脚本 provider 的剧本
    { "toolCalls": [{ "name": "Read", "input": { "file_path": "${cwd}/hello.txt" } }] },
    { "text": "hello.txt 里写的是 hello。" }
  ],
  "followUpTurns": [                   // 可选：同一会话的后续 send
    { "prompt": "再看一次", "calls": 1 }
  ],
  "expect": {                          // 可选：有则比较，无则录制
    "toolCalls": [{ "name": "Read", "input": {}, "ok": true, "output": "hello\n" }],
    "files": { "hello.txt": "<sha256>" },
    "events": [{ "type": "turn.started", "payload": {} }]
  },
  "timeoutMs": 120000                  // 可选：覆盖单 case 超时
}
```

`seed` 与 `script` 中的 `${cwd}` 在运行时替换为 case 的 workspace 绝对路径（与
`mock-provider.mts` 的 `substituteCwd` 同词）。

## 5. 轨迹（trajectory）v1

每个 case 写 `<out>/trajectories/<case>.jsonl`，每行一个 JSON 对象：

1. `{"kind":"meta","schemaVersion":1,"case":...,"prompt":...,"seed":...,"script":...,"followUpTurns":...}`
2. `{"kind":"event","type":...,"payload":...}`（规范化后的会话事件，按实际顺序）
3. `{"kind":"toolCall","name":...,"input":...,"ok":...,"output":...}`
4. `{"kind":"files","files":{路径:sha256}}`
5. `{"kind":"result","status":"passed|failed|recorded","mismatches":[...]}`

规范化规则见 §6；脱敏见 §7。轨迹可直接被 `mode eval replay` 读回，也可由
`toOracleFixture` 转成 `{case,prompt,seed,script,expect}` 的夹具 JSON。

## 6. 规范化（v1 词表）

只做以下替换，其余内容逐字比较：

| 类别 | 规则 |
| --- | --- |
| 易变 id | 键名命中 §6 列表（`sessionId`/`messageId`/`eventId`/`turnId`/`toolCallId`/`requestId`/`spanId`/`traceId`/`callId` 等）→ 值替换为 `<id>` |
| 时间/耗时 | 键名匹配 `timestamp`/`duration`/`elapsedMs`/`*At`/`*Ms` → `<time>` |
| 路径 | case workspace → `<cwd>`；data root → `<data-root>`；系统临时目录 → `<tmp>`（长路径优先） |
| 回环 URL | `http(s)://127.0.0.1|localhost|\[::1\]:<port>` → `<loopback-url>` |
| UUID | 文本里的 UUID 形状 → `<id>` |
| shell 前缀 | `bash: line N:` / `C:\…\bash.exe: line N:` → `<shell>: line <n>:` |
| 无序组 | 连续 `session.updated` 段按稳定串排序后比较；其他事件保持顺序 |
| JSON 文本对象 | 字符串整体是 JSON 对象时按对象键排序后重新序列化 |

工具调用投影（第一层）：`tool.updated` 的 `scheduled` 建条目，`result`/`error` 补
`ok`/`output`；`batch`/`raw`/`progress`/`started` 不产生新条目。输入优先取
`scheduled.input`，缺失时取同 `toolCallId` 的 `model.streaming` 输入。

文件快照（第二层）：workspace 递归列文件，跳过 `.git`、`node_modules`、`.mode-eval`；
内容取 sha256；只比内容，不比 mtime/权限。

## 7. 脱敏

落盘前对轨迹与报告的所有字符串执行：

1. `sk-…`、`Bearer …`、`Authorization:` 头 → `<redacted-key>`；
2. 键名含 `apiKey`/`token`/`secret`/`password` → 值 `<redacted>`；
3. 用户 home → `<home>`；
4. `MODE_EVAL_REDACT_DOMAINS`（逗号分隔）中的域名 → `<redacted-domain>`；
5. 回环 URL → `<loopback-url>`；内部域名不得出现在报告中（含错误文案）。

脱敏是**写盘前**的最后一步；比较用的是脱敏后的值，避免同一 secret 在不同轮次
以不同随机值出现导致假差异。

## 8. 验收场景

1. **确定性**：同一 suite + 脚本 provider 连跑两次，`report.json` 去掉 `timing` 字段后
   逐字节一致；轨迹（去掉 `durationMs`）逐字节一致。
2. **夹具兼容**：`packages/desktop/tests/agent-runtime-oracle/fixtures/read-single.json`
   可直接作为 suite 的单个 case 运行；`toOracleFixture` 产出的形状能被现有 oracle
   回放代码接受。
3. **失败可定位**：篡改 expect 的一个事件/工具输出，报告给出 `path`、`expected`、
   `actual`，退出码 1；未篡改时退出码 0。
4. **录制模式**：无 `expect` 的 case 输出 `recorded`，写出轨迹，不判失败。
5. **脱敏**：case 里注入 `sk-test-…`、`Bearer …`、home 路径、`internal.example` 域名，
   轨迹与报告中均不出现原文。
6. **隔离**：两个 case 串行运行，第二个看不到第一个的 workspace 文件与会话；
   进程退出后无子进程残留。
7. **回放**：`mode eval replay <trajectory.jsonl>` 对同一 agent-server 通过；改动
   agent-server 行为后失败并给出差异。
8. **无网络**：脚本 provider 用例在断网环境可跑通；默认门禁不执行任何真实模型调用。

## 9. 后续扩展（不在首片）

- 真实 provider 小样本与成本统计（需要显式 `--eval-real-provider` 与凭据）；
- pass@k、多次采样、置信区间；
- suite 分片与并行执行（当前串行，保证隔离与确定性）；
- CI opt-in job 与预算上限（首片只落本地命令与文档）。
