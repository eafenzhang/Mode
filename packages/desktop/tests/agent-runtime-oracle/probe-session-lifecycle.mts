// C1 Task 0：黑盒捕获会话生命周期（create → 文本 turn → Read 工具 turn → close → resume →
// read/messages/list），并导出 `db.sqlite` 的 session/message/part/session_input/session_entry 行形状。
//
// 用法：
//   pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-session-lifecycle.mts
//   pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-session-lifecycle.mts --keep
//   pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-session-lifecycle.mts --replay
//
// 纪律（docs/specs/clean-room-policy.md）：只运行旧实现、观察线上帧与数据库行，不读实现体。
// 隔离（tauri-agent-runtime.md §1.4）：driver 同时注入 MODE_DATA_BASE_DIR + HOME/USERPROFILE/MODE_HOME，
// 并断言 `<隔离根>/.mode/.mode-root.json` 与 `<隔离根>/.mode/cli/log` 都存在；真实 `~/.mode` 不参与。
// 期望值只来自本次 oracle 运行：`session-lifecycle-fixtures.mts` 只做投影/规范化，不手写 expect。
//
// 夹具落点：`fixtures/session-lifecycle/`。**不放在 fixtures/ 根下**：fixtures.test.mts 会把根下
// 每个 `*.json` 当作三层行为夹具（`loadFixtures`）并要求 case 名与 `FIXTURE_CASES` 逐字相等；
// 生命周期夹具是方法序/行形状夹具，不是三层行为夹具，放子目录可避免污染那套回放。
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { seedWorkspace, startOracleRuntime } from "./driver.mjs";
import type { MockStep } from "./mock-provider.mjs";
import {
  FIXTURE_DIR,
  capture,
  readDb,
  recordFrames,
  type Capture,
  type JsonObject,
  type PathContext,
} from "./session-lifecycle-fixtures.mjs";

const REPO_ROOT = fileURLToPath(new URL("../../../../", import.meta.url));
const SEED = { "hello.txt": "hello\n" };
const PROMPTS = ["first prompt", "Read hello.txt and summarize"];
const SCRIPT: MockStep[] = [
  { text: "first turn answer" },
  { toolCalls: [{ name: "Read", input: { file_path: "${cwd}/hello.txt" } }] },
  { text: "second turn answer" },
];
const TERMINAL_EVENTS = new Set(["turn.completed", "turn.failed"]);

function delay(ms: number): Promise<void> {
  return new Promise((resolvePromise) => setTimeout(resolvePromise, ms));
}

function terminalCount(events: readonly JsonObject[]): number {
  return events.filter((event) => TERMINAL_EVENTS.has(String(event.type))).length;
}

async function waitForTerminals(
  captureState: Capture,
  expected: number,
  timeoutMs = 90_000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (terminalCount(captureState.events) < expected) {
    if (Date.now() > deadline) {
      throw new Error(
        `oracle 在 ${timeoutMs}ms 内只产生 ${terminalCount(captureState.events)} 个终态`,
      );
    }
    await delay(50);
  }
  await delay(500);
}

function findRequest(requests: readonly JsonObject[], method: string): JsonObject {
  const found = requests.find((request) => request.method === method);
  if (found === undefined) throw new Error(`没有观察到 ${method} 的请求/应答`);
  return found;
}

async function record(): Promise<void> {
  const dataRoot = await mkdtemp(join(tmpdir(), "mode-c1-lifecycle-"));
  const cwd = await mkdtemp(join(tmpdir(), "mode-c1-lifecycle-ws-"));
  const context: PathContext = { dataRoot, cwd, tmpDir: tmpdir() };
  await seedWorkspace(cwd, SEED);
  const captureState = capture();
  const hooks = recordFrames(captureState, context);
  const session = await startOracleRuntime({
    dataRoot,
    cwd,
    script: SCRIPT,
    onRawFrame: hooks.onFrame,
    onRawWrite: hooks.onWrite,
  });
  try {
    captureState.boundaries.afterCreate = captureState.events.length;
    await session.send(PROMPTS[0] as string);
    await waitForTerminals(captureState, 1);
    captureState.boundaries.afterTextTurn = captureState.events.length;
    await session.send(PROMPTS[1] as string);
    await waitForTerminals(captureState, 2);
    captureState.boundaries.afterToolTurn = captureState.events.length;
    await session.request("session/close", { sessionId: session.sessionId });
    await delay(300);
    captureState.boundaries.afterClose = captureState.events.length;
    await session.request("session/resume", { sessionId: session.sessionId });
    await delay(300);
    captureState.boundaries.afterResume = captureState.events.length;
    await session.request("session/read", { sessionId: session.sessionId });
    captureState.boundaries.afterRead = captureState.events.length;
    await session.request("session/messages", { sessionId: session.sessionId });
    captureState.boundaries.afterMessages = captureState.events.length;
    await session.request("session/list", { includeArchived: true });
    captureState.boundaries.afterList = captureState.events.length;

    const db = readDb(join(dataRoot, ".mode", "cli", "db", "db.sqlite"), context);
    const requests = captureState.requests;
    const create = findRequest(requests, "session/create");
    const subscribe = findRequest(requests, "session/subscribe");
    const close = findRequest(requests, "session/close");
    const resume = findRequest(requests, "session/resume");
    const read = findRequest(requests, "session/read");
    const messages = findRequest(requests, "session/messages");
    const list = findRequest(requests, "session/list");
    const input = {
      seed: SEED,
      script: SCRIPT,
      prompts: PROMPTS,
      createParams: create.params,
      subscribeParams: subscribe.params,
      closeParams: close.params,
      resumeParams: resume.params,
      readParams: read.params,
      messagesParams: messages.params,
      listParams: list.params,
    };
    const isolation = {
      dataRootManifest: existsSync(join(dataRoot, ".mode", ".mode-root.json")),
      cliLayoutSentinel: existsSync(join(dataRoot, ".mode", "cli", "log")),
      invalidStdoutLines: session.invalidLines.length,
      mockRequestPaths: [...session.mockRequestPaths],
    };
    const eventsFixture = {
      case: "session-lifecycle-events",
      note:
        "C1 Task 0 oracle 捕获：create → 文本 turn → Read turn → close → resume → read/messages/list。" +
        "expect 全部来自本次运行；session.updated 等并发推送按 raw 顺序记录，见 docs/specs/agent-server-c1-contract.md。",
      oracle: {
        entry: "apps/mode-cli/packages/cli/src/main.ts",
        args: ["app-server", "--stdio"],
        invocation: "node tsx-cli <entry> app-server --stdio",
      },
      input,
      expect: {
        isolation,
        requests,
        reverseRequests: captureState.reverseRequests,
        startupStorage: captureState.startupStorage,
        timeline: captureState.timeline,
        events: captureState.events,
        eventTypes: captureState.events.map((event) => event.type),
        boundaries: captureState.boundaries,
        ignoredNotifications: captureState.ignoredNotifications,
      },
    };
    const dbFixture = {
      case: "session-lifecycle-db",
      note:
        "C1 Task 0 oracle 捕获：同一隔离数据根的 db.sqlite 行形状（session/message/part/session_input/session_entry）。" +
        "JSON 文本列已解析成对象；id/时间/机器路径/工具面已折叠成占位符。",
      input: { seed: SEED, script: SCRIPT, prompts: PROMPTS },
      expect: db,
    };
    await mkdir(FIXTURE_DIR, { recursive: true });
    await writeFile(
      join(FIXTURE_DIR, "session-lifecycle-events.json"),
      `${JSON.stringify(eventsFixture, null, 2)}\n`,
      "utf8",
    );
    await writeFile(
      join(FIXTURE_DIR, "session-lifecycle-db.json"),
      `${JSON.stringify(dbFixture, null, 2)}\n`,
      "utf8",
    );
    const rowCounts = Object.fromEntries(
      Object.entries(db.tables as JsonObject).map(([table, value]) => [
        table,
        (value as JsonObject).rowCount,
      ]),
    );
    process.stdout.write(
      `${JSON.stringify(
        {
          dataRoot,
          cwd,
          requests: requests.map((request) => request.method),
          eventTypes: eventsFixture.expect.eventTypes,
          rowCounts,
          fixtureDir: FIXTURE_DIR,
        },
        null,
        2,
      )}\n`,
    );
  } finally {
    await session.stop();
    if (!process.argv.includes("--keep")) {
      await rm(dataRoot, { recursive: true, force: true });
      await rm(cwd, { recursive: true, force: true });
    }
  }
}

function resolveAgentServerBinary(): string | undefined {
  const configured = process.env.MODE_AGENT_SERVER_COMMAND?.trim();
  if (configured) return configured;
  for (const name of ["agent-server", "mode-agent"]) {
    for (const profile of ["debug", "release"]) {
      for (const suffix of ["", ".exe"]) {
        const candidate = join(REPO_ROOT, "target", profile, `${name}${suffix}`);
        if (existsSync(candidate)) return candidate;
      }
    }
  }
  return undefined;
}

/** Step 3：回放用例。C1 的 `agent-server` 尚未实现时，这里必须以「binary 不存在」为红。 */
async function replay(): Promise<void> {
  const fixturePath = join(FIXTURE_DIR, "session-lifecycle-events.json");
  const fixture = JSON.parse(await readFile(fixturePath, "utf8")) as {
    expect: { requests: { method: string }[] };
  };
  const binary = resolveAgentServerBinary();
  if (binary === undefined) {
    throw new Error(
      "回放为红：agent-server 二进制不存在（MODE_AGENT_SERVER_COMMAND 未设置，target/{debug,release}/" +
        "{agent-server,mode-agent} 均不存在）；C1 Task 1 之后才会产出该二进制",
    );
  }
  process.stdout.write(
    `agent-server=${binary}\nfixture=${fixturePath}\n` +
      `expectedMethods=${fixture.expect.requests.map((request) => request.method).join(",")}\n` +
      "Task 0 只校验二进制可拉起与 fixture 可读；三层回放由 C1 Task 10 接管。\n",
  );
  const child = spawn(binary, ["app-server", "--stdio"], { cwd: REPO_ROOT, stdio: "pipe" });
  await new Promise<void>((resolvePromise, rejectPromise) => {
    child.once("error", rejectPromise);
    child.once("spawn", () => resolvePromise());
  });
  child.stdin?.end();
  await new Promise<void>((resolvePromise) => child.once("exit", () => resolvePromise()));
}

/** 补充观察：session_input 在 persistence/inputId 变体下是否写入（不产出夹具）。 */
async function inputVariants(): Promise<void> {
  const dataRoot = await mkdtemp(join(tmpdir(), "mode-c1-input-variants-"));
  const cwd = await mkdtemp(join(tmpdir(), "mode-c1-input-variants-ws-"));
  const context: PathContext = { dataRoot, cwd, tmpDir: tmpdir() };
  await seedWorkspace(cwd, SEED);
  const session = await startOracleRuntime({ dataRoot, cwd, script: [{ text: "variant answer" }] });
  try {
    const variants: { name: string; persistence?: string; send: Record<string, string> }[] = [
      { name: "deferred+inputId", persistence: "deferred", send: { inputId: "input_deferred_1" } },
      {
        name: "default+inputId+queryId",
        send: { inputId: "input_default_1", queryId: "query_1" },
      },
      { name: "immediate", persistence: "immediate", send: {} },
    ];
    for (const variant of variants) {
      const created = (await session.request("session/create", {
        workspace: { workspacePath: cwd, workspaceKey: cwd },
        model: {
          providerId: "oracle-mock",
          modelId: "mock-1",
          options: { reasoningLevel: "disabled" },
        },
        thoughtLevel: "disabled",
        titleGenerationEnabled: false,
        ...(variant.persistence === undefined ? {} : { persistence: variant.persistence }),
      })) as { session: { sessionId: string } };
      await session.request("session/subscribe", {
        sessionId: created.session.sessionId,
        deliveryKind: "desktop-continuous",
        includeSnapshot: true,
      });
      await session.request("session/send", {
        sessionId: created.session.sessionId,
        content: `variant ${variant.name}`,
        ...variant.send,
      });
      await delay(500);
      await session.request("session/close", { sessionId: created.session.sessionId });
    }
    const db = readDb(join(dataRoot, ".mode", "cli", "db", "db.sqlite"), context);
    const input = (db.tables as JsonObject).session_input as { rowCount: number };
    process.stdout.write(
      `${JSON.stringify(
        {
          variants: variants.map((variant) => variant.name),
          totalSessionInputRows: input.rowCount,
        },
        null,
        2,
      )}\n`,
    );
  } finally {
    await session.stop();
    await rm(dataRoot, { recursive: true, force: true });
    await rm(cwd, { recursive: true, force: true });
  }
}

if (process.argv.includes("--replay")) {
  await replay().catch((error: unknown) => {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
    process.exitCode = 1;
  });
} else if (process.argv.includes("--input-variants")) {
  await inputVariants();
} else {
  await record();
}
