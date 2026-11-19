// LT-4 公开入口：suite 加载、评测执行、轨迹与回放、agent-server 驱动与 CLI 命令。
export {
  createAgentServerSessionFactory,
  type AgentServerLaunch,
  type EvalSession,
  type EvalSessionEvent,
  type SessionFactory,
  type SessionStartInput,
} from "./agent-server.js";
export {
  EvalConfigError,
  resolveAgentServerLaunch,
  runEvalCommand,
  type EvalCommandContext,
  type EvalCommandOptions,
} from "./cli.js";
export {
  normalizeValue,
  projectEventStream,
  projectToolCalls,
  stableStringify,
  type NormalizeContext,
  type ProjectedEvent,
} from "./normalize.js";
export { createRedactor, type Redactor, type RedactorOptions } from "./redact.js";
export {
  buildReport,
  formatReportText,
  stripTiming,
  type ReportWithoutTiming,
} from "./report.js";
export { compareObserved, replayTrajectory, runSingleCase, runSuite } from "./runner.js";
export { loadSuite, SuiteLoadError } from "./suite.js";
export { takeFileSnapshot } from "./snapshot.js";
export {
  readTrajectory,
  toOracleFixture,
  trajectoryFileName,
  writeTrajectory,
  type Trajectory,
  type TrajectoryLine,
  type TrajectoryMeta,
} from "./trajectory.js";
export {
  CaseSchema,
  ObservedSchema,
  ScriptStepSchema,
  SuiteSchema,
  TRAJECTORY_SCHEMA_VERSION,
  type CaseResult,
  type CaseStatus,
  type EvalCase,
  type EvalEvent,
  type FollowUpTurn,
  type GeneratedSeed,
  type Mismatch,
  type Observed,
  type RunReport,
  type ScriptStep,
  type ScriptToolCall,
  type Suite,
  type ToolCall,
} from "./types.js";
