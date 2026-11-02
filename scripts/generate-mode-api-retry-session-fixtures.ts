/* oxlint-disable eslint(max-lines) -- 对拍脚本集中声明会话重试/草稿矩阵，拆分会降低可核对性。 */
/**
 * 生成 `mode-api-retry` 会话级 tracker / draft registry 的对拍数据。
 *
 * 真源（clean-room：只读本仓库 TS）：
 * - `packages/services/src/mode-session/modeSessionApiRetry.ts`
 * - `packages/services/src/mode-session/modeSessionDraftRegistry.ts`
 *
 * ```bash
 * pnpm mode-api-retry-session:fixtures
 * cargo test -p mode-api-retry
 * ```
 */
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { createModeSessionApiRetryRuntimeTracker } from "../packages/services/src/mode-session/modeSessionApiRetry.js";
import { createModeDeferredDraftRegistry } from "../packages/services/src/mode-session/modeSessionDraftRegistry.js";

const ROOT_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUTPUT = path.join(
  ROOT_DIR,
  "crates",
  "mode-api-retry",
  "fixtures",
  "mode-api-retry-session.json",
);

function snapshot(over: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    session: {
      sessionId: "s1",
      status: "running",
      workspace: { workspacePath: "/ws/a" },
      ...over,
    },
  };
}

interface TrackerStep {
  kind: "event" | "snapshot";
  event?: unknown;
  snapshot?: unknown;
}

interface TrackerCase {
  name: string;
  target: Record<string, unknown>;
  steps: TrackerStep[];
}

function buildTrackerCases(): TrackerCase[] {
  const target = { workspacePath: "/ws/a", sessionId: "s1" };
  return [
    {
      name: "direct-api-retry-then-clear-on-complete",
      target,
      steps: [
        {
          kind: "event",
          event: {
            type: "session.event",
            event: {
              type: "session.updated",
              sessionId: "s1",
              payload: { apiRetry: { attempt: 2, maxAttempts: 4, error: "e" } },
            },
          },
        },
        { kind: "snapshot", snapshot: snapshot() },
        { kind: "snapshot", snapshot: snapshot({ status: "completed" }) },
      ],
    },
    {
      name: "stream-recovery",
      target,
      steps: [
        {
          kind: "event",
          event: {
            type: "session.event",
            event: {
              type: "streamRecovery.updated",
              sessionId: "s1",
              payload: { retryNumber: 2, maxRetries: 5, message: "m" },
            },
          },
        },
        { kind: "snapshot", snapshot: snapshot() },
      ],
    },
    {
      name: "stream-recovery-nested-ignored",
      target,
      steps: [
        {
          kind: "event",
          event: {
            type: "session.event",
            event: {
              type: "streamRecovery.updated",
              sessionId: "s1",
              payload: { streamRecovery: { retryNumber: 2 } },
            },
          },
        },
        { kind: "snapshot", snapshot: snapshot() },
      ],
    },
    {
      name: "progress-clears",
      target,
      steps: [
        {
          kind: "event",
          event: {
            type: "session.event",
            event: {
              type: "session.updated",
              sessionId: "s1",
              payload: { apiRetry: { attempt: 1 } },
            },
          },
        },
        {
          kind: "event",
          event: {
            type: "session.event",
            event: {
              type: "session.updated",
              sessionId: "s1",
              payload: { kind: "text_delta", delta: "x" },
            },
          },
        },
        { kind: "snapshot", snapshot: snapshot() },
      ],
    },
    {
      name: "empty-progress-keeps",
      target,
      steps: [
        {
          kind: "event",
          event: {
            type: "session.event",
            event: {
              type: "session.updated",
              sessionId: "s1",
              payload: { apiRetry: { attempt: 1 } },
            },
          },
        },
        {
          kind: "event",
          event: {
            type: "session.event",
            event: {
              type: "session.updated",
              sessionId: "s1",
              payload: { kind: "text_delta", delta: "" },
            },
          },
        },
        { kind: "snapshot", snapshot: snapshot() },
      ],
    },
    {
      name: "runtime-payload",
      target,
      steps: [
        {
          kind: "event",
          event: {
            type: "session.event",
            event: {
              type: "session.updated",
              sessionId: "s1",
              payload: { runtime: { apiRetry: { nextAttempt: 5 } } },
            },
          },
        },
        { kind: "snapshot", snapshot: snapshot() },
      ],
    },
    {
      name: "meta-payload",
      target,
      steps: [
        {
          kind: "event",
          event: {
            type: "session.event",
            event: {
              type: "session.updated",
              sessionId: "s1",
              payload: { _meta: { mode: { apiRetry: { attempt: 1 } } } },
            },
          },
        },
        { kind: "snapshot", snapshot: snapshot() },
      ],
    },
    {
      name: "model-retry-scheduled",
      target,
      steps: [
        {
          kind: "event",
          event: {
            type: "session.event",
            event: {
              type: "session.updated",
              sessionId: "s1",
              payload: { type: "model_retry_scheduled", attempt: 2 },
            },
          },
        },
        { kind: "snapshot", snapshot: snapshot() },
      ],
    },
    {
      name: "snapshot-existing-api-retry",
      target,
      steps: [
        {
          kind: "snapshot",
          snapshot: {
            session: {
              sessionId: "s1",
              status: "running",
              workspace: { workspacePath: "/ws/a" },
            },
            runtime: { apiRetry: { attempt: 3 } },
          },
        },
        { kind: "snapshot", snapshot: snapshot() },
      ],
    },
    {
      name: "session-isolation",
      target,
      steps: [
        {
          kind: "event",
          event: {
            type: "session.event",
            event: {
              type: "session.updated",
              sessionId: "s2",
              payload: { apiRetry: { attempt: 1 } },
            },
          },
        },
        { kind: "snapshot", snapshot: snapshot() },
        { kind: "snapshot", snapshot: snapshot({ sessionId: "s2" }) },
      ],
    },
    {
      name: "workspace-identity-mismatch",
      target: { workspacePath: "/ws/a", workspaceIdentity: "id-a", sessionId: "s1" },
      steps: [
        {
          kind: "event",
          event: {
            type: "session.event",
            event: {
              type: "session.updated",
              sessionId: "s1",
              payload: { apiRetry: { attempt: 1 } },
            },
          },
        },
        { kind: "snapshot", snapshot: snapshot() },
        {
          kind: "snapshot",
          snapshot: snapshot({
            workspace: { workspacePath: "/ws/a", workspaceIdentity: "id-a" },
          }),
        },
      ],
    },
    {
      name: "non-session-event",
      target,
      steps: [{ kind: "event", event: { type: "message", payload: { hello: 1 } } }],
    },
    {
      name: "snapshot-missing-session",
      target,
      steps: [{ kind: "snapshot", snapshot: { session: {} } }],
    },
  ];
}

interface DraftStep {
  kind: "remember" | "has" | "forget";
  params?: Record<string, unknown>;
  snapshot?: Record<string, unknown>;
  target?: Record<string, unknown>;
}

function buildDraftCases(): Array<{ name: string; steps: DraftStep[] }> {
  return [
    {
      name: "remember-by-snapshot-identity",
      steps: [
        {
          kind: "remember",
          params: { workspacePath: "/ws/a", sessionId: "s1" },
          snapshot: snapshot({
            workspace: { workspacePath: "/ws/a", workspaceIdentity: "id-a" },
          }),
        },
        {
          kind: "has",
          target: { workspacePath: "/ws/a", workspaceIdentity: "id-a", sessionId: "s1" },
        },
        { kind: "has", target: { workspacePath: "/ws/a", sessionId: "s1" } },
        {
          kind: "forget",
          target: { workspacePath: "/ws/a", workspaceIdentity: "id-a", sessionId: "s1" },
        },
        {
          kind: "has",
          target: { workspacePath: "/ws/a", workspaceIdentity: "id-a", sessionId: "s1" },
        },
      ],
    },
    {
      name: "params-identity-fallback",
      steps: [
        {
          kind: "remember",
          params: { workspacePath: "/ws/a", workspaceIdentity: "id-b", sessionId: "s1" },
          snapshot: snapshot(),
        },
        {
          kind: "has",
          target: { workspacePath: "/ws/a", workspaceIdentity: "id-b", sessionId: "s1" },
        },
      ],
    },
    {
      name: "trim-identity",
      steps: [
        {
          kind: "remember",
          params: { workspacePath: "/ws/a", sessionId: "s1" },
          snapshot: snapshot({
            workspace: { workspacePath: "/ws/a", workspaceIdentity: "id-a" },
          }),
        },
        {
          kind: "has",
          target: { workspacePath: "/ws/a", workspaceIdentity: " id-a ", sessionId: "s1" },
        },
      ],
    },
    {
      name: "different-session",
      steps: [
        {
          kind: "remember",
          params: { workspacePath: "/ws/a", sessionId: "s1" },
          snapshot: snapshot({
            workspace: { workspacePath: "/ws/a", workspaceIdentity: "id-a" },
          }),
        },
        {
          kind: "has",
          target: { workspacePath: "/ws/a", workspaceIdentity: "id-a", sessionId: "s2" },
        },
      ],
    },
  ];
}

function main(): void {
  const tracker = buildTrackerCases().map((testCase) => {
    const runtime = createModeSessionApiRetryRuntimeTracker();
    return {
      name: testCase.name,
      target: testCase.target,
      steps: testCase.steps.map((step) => {
        const result =
          step.kind === "event"
            ? runtime.trackApiRetryFromSessionEvent(testCase.target as never, step.event as never)
            : runtime.withApiRetryRuntime(step.snapshot as never);
        return {
          kind: step.kind,
          ...(step.kind === "event" ? { event: step.event } : { snapshot: step.snapshot }),
          expected: JSON.parse(JSON.stringify(result)) as unknown,
        };
      }),
    };
  });
  const draft = buildDraftCases().map((testCase) => {
    const registry = createModeDeferredDraftRegistry();
    return {
      name: testCase.name,
      steps: testCase.steps.map((step) => {
        if (step.kind === "remember") {
          registry.remember(step.params as never, step.snapshot as never);
          return { kind: step.kind, params: step.params, snapshot: step.snapshot, expected: null };
        }
        if (step.kind === "forget") {
          registry.forget(step.target as never);
          return { kind: step.kind, target: step.target, expected: null };
        }
        return {
          kind: step.kind,
          target: step.target,
          expected: registry.has(step.target as never),
        };
      }),
    };
  });
  const fixture = { tracker, draft };
  mkdirSync(path.dirname(OUTPUT), { recursive: true });
  writeFileSync(OUTPUT, `${JSON.stringify(fixture, null, 2)}\n`);
  process.stdout.write(`wrote ${path.relative(ROOT_DIR, OUTPUT)}\n`);
}

main();
