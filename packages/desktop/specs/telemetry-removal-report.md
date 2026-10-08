# 桌面遥测移除报告

已删除 ARMS 初始化、使用事件上报、崩溃采集、周期资源/网络采样、Host/Scheduler 转发、Renderer IPC 桥以及 OTLP 操作轨迹与 TTFT 导出。共享平台要求的两个 report 方法保留为空操作，不执行 IPC 或网络请求。

本地 logger、用户主动反馈及日志导出、正常业务请求、资源管理器按需查询、数据库启动、窗口/Host/远程 attachment 业务路径保留。未执行 git commit。

## 验证

- 根目录 `pnpm typecheck`：通过（退出码 0），各修改组均执行；未修改根 typecheck 脚本或放宽类型规则。
- 根目录 `pnpm lint`：通过（退出码 0），62 warnings，0 errors。
- `node --test packages/desktop/tests/no-telemetry.test.mjs`：4/4 通过。
- `architecture:check --changed`：0 violations，0 new。
- 锁文件 Desktop 依赖与 package.json 一致；patch 清单一致，只删除 ARMS patch，其他 patch 保留。
- `git diff --check`：通过。

## 验证边界与需人工关注

- 本机实际验证工具为 Node 24.16.0、pnpm 10.18.0；mise.toml 指定 24.14.0/10.33.2，但 mise 不可用且自动下载受限。验证使用临时 PATH 选择现有 pnpm，并关闭自动版本下载；没有修改仓库工具版本配置。
- 基线 freshness 命令已执行，但 git fetch 无法写入只读 .git/FETCH_HEAD；无法确认远端最新基线。
- 根 typecheck 仅包含 Desktop Host。额外检查 Main/preload/renderer/scheduler 的完整桌面配置发现原有类型问题；对 HEAD 虚拟源码基线对比，去重诊断从 67 降至 58，新增 0。没有顺带修复无关问题。
- 未启动 Electron 或抓包；数据库、OAuth、远程连接测试使用真实模块配合 Electron 边界替身，不能代替真实应用 E2E。
- 按任务中的保守规则整体删除崩溃模块，因此自定义 dump 落盘/归档、OOM 注解、保留清理和周期内存诊断也取消；未删除用户已有日志、dump 或配置文件。
- 保留 deviceMid 作为业务请求、反馈和本地身份所需数据，兼容原 telemetry-state.json 文件名；这不再驱动遥测上报。
- 清理范围是 Desktop 及直接相关依赖/锁文件。独立 CLI、远端 Server 和其他包的遥测实现未删除；Desktop 不再订阅转发它们的遥测，并剔除传给本地 Host 的 OTLP/遥测环境配置。远端独立进程仍需单独审计。

## 删除文件（65）

- `packages/desktop/src/host/hostAgentResourceTelemetry.ts`
- `packages/desktop/src/host/hostMcpResourceTelemetry.ts`
- `packages/desktop/src/host/hostMcpTelemetry.ts`
- `packages/desktop/src/host/hostNetworkTelemetry.ts`
- `packages/desktop/src/host/hostResourceTelemetryEnvironment.ts`
- `packages/desktop/src/host/hostSelfResourceTelemetry.ts`
- `packages/desktop/src/host/hostServiceResourceTelemetry.ts`
- `packages/desktop/src/host/hostSessionCreateTelemetry.ts`
- `packages/desktop/src/host/hostToolExecResourceTelemetry.ts`
- `packages/desktop/src/main/appARMSBootstrap.ts`
- `packages/desktop/src/main/appCrashCaptureBootstrap.ts`
- `packages/desktop/src/main/appLaunchCoordinator.ts`
- `packages/desktop/src/main/appTelemetryRuntime.ts`
- `packages/desktop/src/main/armsBrowserPerfLoadNudge.ts`
- `packages/desktop/src/main/armsEventRedaction.ts`
- `packages/desktop/src/main/armsUserIdentity.ts`
- `packages/desktop/src/main/crashDumpAnnotations.ts`
- `packages/desktop/src/main/databaseStartupTelemetry.ts`
- `packages/desktop/src/main/desktopArmsCustomEvent.ts`
- `packages/desktop/src/main/desktopCrashCapture.ts`
- `packages/desktop/src/main/desktopMcpTelemetry.ts`
- `packages/desktop/src/main/desktopNetworkTelemetry.ts`
- `packages/desktop/src/main/desktopRemoteUsageArmsTelemetry.ts`
- `packages/desktop/src/main/desktopResourceTelemetry.ts`
- `packages/desktop/src/main/desktopStabilityTelemetry.ts`
- `packages/desktop/src/main/desktopTelemetryFetch.ts`
- `packages/desktop/src/main/desktopZCodeDataSizeTelemetry.ts`
- `packages/desktop/src/main/localTtftExportDedupe.ts`
- `packages/desktop/src/main/localTtftExporter.ts`
- `packages/desktop/src/main/localTtftSpans.ts`
- `packages/desktop/src/main/longTaskAttributionSummary.ts`
- `packages/desktop/src/main/mainMemoryDiagnostics.ts`
- `packages/desktop/src/main/networkTelemetryAggregator.ts`
- `packages/desktop/src/main/processResourceAppTotals.ts`
- `packages/desktop/src/main/processResourceChromiumSource.ts`
- `packages/desktop/src/main/processResourceCliSource.ts`
- `packages/desktop/src/main/processResourceExternalAppSamples.ts`
- `packages/desktop/src/main/processResourceMcpTelemetrySource.ts`
- `packages/desktop/src/main/processResourceRendererHeapSource.ts`
- `packages/desktop/src/main/processResourceRoleClassifier.ts`
- `packages/desktop/src/main/processResourceSampleSourceRegistry.ts`
- `packages/desktop/src/main/processResourceSampleSources.ts`
- `packages/desktop/src/main/processResourceSelfHeapSource.ts`
- `packages/desktop/src/main/processResourceSystemSource.ts`
- `packages/desktop/src/main/processResourceSystemWindowAggregator.ts`
- `packages/desktop/src/main/processResourceSystemWindowEvent.ts`
- `packages/desktop/src/main/processResourceWindowAggregator.ts`
- `packages/desktop/src/main/processResourceWindowEvent.ts`
- `packages/desktop/src/main/rendererActionTraceBroker.ts`
- `packages/desktop/src/main/rendererActionTraceExporter.ts`
- `packages/desktop/src/main/rendererActionTraceIpc.ts`
- `packages/desktop/src/main/rendererActionTraceRollout.ts`
- `packages/desktop/src/main/startupTelemetryDelivery.ts`
- `packages/desktop/src/main/modeDataSizeTelemetryState.ts`
- `packages/desktop/src/renderer/appTelemetryBridge.ts`
- `packages/desktop/src/renderer/src/localTtftBootstrap.ts`
- `packages/desktop/src/renderer/src/userActionTraceBootstrap.ts`
- `packages/desktop/src/scheduler/schedulerResourceTelemetry.ts`
- `packages/desktop/src/shared/armsRumBridgeForward.ts`
- `packages/desktop/src/shared/armsRumShared.d.ts`
- `packages/desktop/src/shared/armsRumShared.d.ts.map`
- `packages/desktop/src/shared/armsRumShared.js`
- `packages/desktop/src/shared/armsRumShared.js.map`
- `packages/desktop/src/shared/armsRumShared.ts`
- `patches/@arms__rum-electron@0.0.3.patch`

## 修改文件（22）

- `package.json`
- `packages/desktop/electron-builder.config.js`
- `packages/desktop/package.json`
- `packages/desktop/scripts/bundle.mjs`
- `packages/desktop/src/host/index.ts`
- `packages/desktop/src/main/databaseStartupRelay.ts`
- `packages/desktop/src/main/desktopCronScheduler.ts`
- `packages/desktop/src/main/desktopDeviceMid.ts`
- `packages/desktop/src/main/desktopHostProcess.ts`
- `packages/desktop/src/main/desktopMainIpcRemote.ts`
- `packages/desktop/src/main/desktopRemoteSessions.ts`
- `packages/desktop/src/main/desktopRuntimeEnv.ts`
- `packages/desktop/src/main/desktopWindowLifecycle.ts`
- `packages/desktop/src/main/index.ts`
- `packages/desktop/src/main/resourceManagerWindow.ts`
- `packages/desktop/src/main/singleFeatureRollout.ts`
- `packages/desktop/src/preload/index.ts`
- `packages/desktop/src/renderer/src/desktopPlatform.ts`
- `packages/desktop/src/renderer/src/main.tsx`
- `packages/desktop/src/scheduler/index.ts`
- `packages/desktop/tsconfig.preload.json`
- `pnpm-lock.yaml`

## 新增文件（4）

- `packages/desktop/specs/no-telemetry.md`
- `packages/desktop/specs/telemetry-removal-report.md`
- `packages/desktop/src/main/desktopTelemetryPolicy.ts`
- `packages/desktop/tests/no-telemetry.test.mjs`
