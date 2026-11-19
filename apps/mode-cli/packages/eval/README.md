# @mode/eval

LT-4：CLI 评测与轨迹记录（`mode eval run|replay`）。

- 规格：`docs/specs/cli-eval-trajectory.md`
- 计划：`docs/plans/2026-11-19-lt4-cli-eval-trajectory.md`

## 用法

```sh
# 跑一个 suite（单文件或目录；目录模式下每个含 prompt+script 的 *.json 是一个 case）
mode eval run packages/desktop/tests/agent-runtime-oracle/fixtures/read-single.json

# 产物：<cwd>/.mode-eval/trajectories/<case>.jsonl 与 <cwd>/.mode-eval/report.json
# 回放一条轨迹
mode eval replay .mode-eval/trajectories/read-single.jsonl
```

agent-server 解析优先级：`--eval-agent-server` → `MODE_EVAL_AGENT_SERVER_COMMAND` →
`MODE_AGENT_SERVER_COMMAND`（+`MODE_AGENT_SERVER_ARGS_JSON`）→ 向上找
`target/{debug,release}/agent-server(.exe)`；都找不到时退出码 1，错误码
`MODE_EVAL_AGENT_SERVER_UNAVAILABLE`。

脚本 provider 由 Rust agent-server 的 `MODE_AGENT_SERVER_SCRIPTED_PROVIDER` 提供
（测试专用开关，生产路径不存在）；评测默认不联网、不进默认门禁。
