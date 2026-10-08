# CLI 与共享层遥测移除报告

已删除 CLI OTLP/metrics/trace/model API 上报包与生命周期装配、telemetryCore、Host 遥测身份注入、shared 遥测配置捕获与无人引用的启用常量；删除 TTFT recorder、独立通知和 services 接收器。日志、主动反馈和正常模型请求保留。

## 验证

| 检查 | 结果 |
| --- | --- |
| 根 `pnpm typecheck` | 通过，退出码 0 |
| 根 `pnpm lint` | 通过，0 errors、63 warnings |
| CLI `pnpm --dir apps/mode-cli typecheck` | 通过，25/25 tasks |
| CLI `pnpm --dir apps/mode-cli build` | 通过，15/15 tasks；browser-use-plugin 有既有 outputs 配置警告 |
| CLI `pnpm --dir apps/mode-cli lint` | 未通过，既有 max-lines 错误；逐包同版本 oxlint 对比 Git HEAD 副本：85 → 83 errors，没有新增错误位置 |
| `node --test apps/mode-cli/tests/no-telemetry.test.mjs packages/desktop/tests/no-telemetry.test.mjs` | 8/8 通过 |
| `pnpm architecture:check --changed` | 0 violations / 0 new |
| `git diff --check` | 通过 |

本机默认 pnpm 启动停滞；通过临时 PATH 使用本机已有 pnpm 10.33.2，并将根 node_modules/.bin 加入 PATH，解决 CLI 独立入口找不到 turbo 的问题。未修改仓库工具链配置。实际 Node 为 24.16.0，指定的 24.14.0 未安装；精确指定版本尚未验证。workspace freshness 的 git fetch 因沙箱禁止写 .git/FETCH_HEAD 失败，未确认远端最新基线。

## 必须说明的保留边界

- `conversationTelemetryFact` 仍被 `packages/mode-server-cli/src/server-core/taskActivityTracker.ts` 用于运行任务数统计；UI 的 `ConversationTelemetryAttachment.tsx` 仍导入 `createConversationTelemetryService`。因此保留 v4/telemetry/event、shared telemetry.ts、CLI normalizer/发送端与 services 订阅/转发/清理。该链路为本地业务事实，不初始化外部上报 SDK。若必须连名称和协议一起删除，需要另行迁移任务统计与 UI 消费。
- conversationTelemetry 服务包装为仍被使用的公开 API，添加中文注释说明其无网络上报行为。TTFT 的公开订阅接口仍被 UI 引用，改为 Event.None，删除其内部 emitter 与消息处理，不留下采集状态。
- 核心运行时 tracing 端口保留现有无 IO 默认实现并加中文注释；workspace hook 本地日志、MCP 资源查询保留。
- 未启动真实桌面/手机交互或发起真实模型请求。本轮证明范围为类型、构建、lint、删除边界和环境清洗回归。
- 未执行 git commit，未修改 README/site，保留已有桌面端与根清单改动。pnpm-lock.yaml 仅在已有改动上移除 telemetry importer 与 bootstrap 依赖；CLI 独立锁文件原本没有 telemetry importer，未重写它。第三方清单仅移除已删除包的 manifest 条目，未重新生成全量库存。

## 文件清单

以下为本轮修改/删除；pnpm-lock.yaml 同时包含上一轮已有改动。D=删除，M=修改，A=新增。

- `M` `apps/mode-cli/packages/bootstrap/package.json`
- `M` `apps/mode-cli/packages/bootstrap/src/app/create-app.ts`
- `M` `apps/mode-cli/packages/bootstrap/src/app/script-workflow-child-runtime.ts`
- `M` `apps/mode-cli/packages/bootstrap/src/app/types.ts`
- `M` `apps/mode-cli/packages/bootstrap/src/app/workflow-facade.ts`
- `M` `apps/mode-cli/packages/bootstrap/src/index.ts`
- `D` `apps/mode-cli/packages/bootstrap/src/telemetry-bootstrap.ts`
- `M` `apps/mode-cli/packages/bootstrap/src/mode-protocol-entrypoint.ts`
- `D` `apps/mode-cli/packages/bootstrap/src/mode-protocol-v4/local-ttft-clock.ts`
- `D` `apps/mode-cli/packages/bootstrap/src/mode-protocol-v4/local-ttft-compaction.ts`
- `D` `apps/mode-cli/packages/bootstrap/src/mode-protocol-v4/local-ttft.ts`
- `M` `apps/mode-cli/packages/bootstrap/src/mode-protocol-v4/v4-gateway.ts`
- `M` `apps/mode-cli/packages/bootstrap/src/mode-protocol/runtime-cleanup.ts`
- `M` `apps/mode-cli/packages/bootstrap/src/mode-protocol/v4-bridge.ts`
- `M` `apps/mode-cli/packages/cli/src/cli-types.ts`
- `M` `apps/mode-cli/packages/cli/src/prompt-command.ts`
- `M` `apps/mode-cli/packages/cli/src/tui-prompt-handler-runtime.ts`
- `M` `apps/mode-cli/packages/cli/src/tui-prompt-handler.ts`
- `M` `apps/mode-cli/packages/core/src/telemetry/runtime-telemetry.ts`
- `D` `apps/mode-cli/packages/telemetry/package.json`
- `D` `apps/mode-cli/packages/telemetry/src/agent-metrics.ts`
- `D` `apps/mode-cli/packages/telemetry/src/agent-trace-runtime.ts`
- `D` `apps/mode-cli/packages/telemetry/src/agent-trace-support.ts`
- `D` `apps/mode-cli/packages/telemetry/src/bootstrap.ts`
- `D` `apps/mode-cli/packages/telemetry/src/compatibility-adapters.ts`
- `D` `apps/mode-cli/packages/telemetry/src/error-sanitizer.ts`
- `D` `apps/mode-cli/packages/telemetry/src/index.ts`
- `D` `apps/mode-cli/packages/telemetry/src/model-api-recorder.ts`
- `D` `apps/mode-cli/packages/telemetry/src/otlp-exporter.ts`
- `D` `apps/mode-cli/packages/telemetry/src/provider-endpoint.ts`
- `D` `apps/mode-cli/packages/telemetry/tsconfig.json`
- `D` `packages/desktop/scripts/ttft-otlp-receiver.mjs`
- `M` `packages/services/src/conversation-telemetry/conversationTelemetry.ts`
- `M` `packages/services/src/node.ts`
- `D` `packages/services/src/telemetry/telemetryCore.ts`
- `D` `packages/services/src/mode-agent/agentTelemetryEnv.ts`
- `M` `packages/services/src/mode-agent/modeAgentService.ts`
- `M` `packages/shared/src/env.ts`
- `M` `packages/shared/src/index.ts`
- `M` `packages/shared/src/runtimeEnv.ts`
- `M` `packages/shared/src/telemetryRedaction.ts`
- `M` `packages/shared/src/mode-protocol-v4/transport.ts`
- `M` `pnpm-lock.yaml`
- `M` `scripts/build-desktop-agent-cli.mjs`
- `M` `third-party/inventory.json`
- `A` `apps/mode-cli/specs/no-telemetry.md`
- `A` `apps/mode-cli/specs/telemetry-removal-report.md`
- `A` `apps/mode-cli/tests/no-telemetry.test.mjs`
