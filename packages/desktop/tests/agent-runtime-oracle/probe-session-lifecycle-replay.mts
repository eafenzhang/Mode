// C1 Task 10：会话生命周期探针的**方法序回放**（`probe-session-lifecycle.mts --replay` 的执行体）。
//
// 拆出本文件是为了守住 oxlint 的 `max-lines`（400 行，错误级），不是第二套纪律。
//
// 用 Rust `agent-server` 按 oracle 夹具的**同一方法序**走一遍生命周期
// （create → subscribe → send ×2 → close → resume → read → messages → list），
// 校验三件事：
//   1. 9 个宿主方法全部成功（无 `-32601` / `-32603`）；
//   2. 反向请求的 scope 序列符合「每 materialize 一对」的 oracle 观察——其中
//      resume 路径的 `user-execution` 时机差（oracle resume 后立即成对；C1 延到
//      首个 send）是**已登记偏差**（`tauri-agent-runtime.md` §C1.4-1），
//      这里显式放行并打印，不算红；
//   3. DB 行数对得上 oracle 观察减去已登记的缺口：合成 timeline 消息行与其
//      timeline part（oracle +1/+1）、`runtime/bash_shell_selection` entry
//      （oracle +1，shell 检测未建模）——见 §C1.3，同样显式放行并打印。
//
// 剧本来自夹具 `input.script`（`${cwd}` 占位符由 Rust scripted provider 替换），
// 经 `MODE_AGENT_SERVER_SCRIPTED_PROVIDER` 注入；三层逐载荷对拍归
// `agent-server-replay.test.mts`，这里只钉方法序与行数。
import assert from "node:assert/strict";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { seedWorkspace } from "./driver.mjs";
import { ensureAgentServerBinary } from "./agent-server-env.mjs";
import { startAgentServer, type AgentServerSession } from "./agent-server-session.mjs";
import { readDb, FIXTURE_DIR, type JsonObject, type PathContext } from "./session-lifecycle-fixtures.mjs";

const REPO_ROOT = fileURLToPath(new URL("../../../../", import.meta.url));
const SEED = { "hello.txt": "hello\n" };
const TERMINAL_EVENTS = new Set(["turn.completed", "turn.failed"]);

function delay(ms: number): Promise<void> {
  return new Promise((resolvePromise) => setTimeout(resolvePromise, ms));
}

/** 回放侧的终态等待：数 `session.frames`（harness 的未映射帧）里的终态。 */
async function waitForReplayTerminals(
  session: AgentServerSession,
  expected: number,
  timeoutMs = 90_000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  const count = (): number =>
    session.frames.filter((frame) => TERMINAL_EVENTS.has(frame.type)).length;
  while (count() < expected) {
    if (Date.now() > deadline) {
      throw new Error(
        `agent-server 在 ${timeoutMs}ms 内只产生 ${count()} 个终态；stderr=${session.stderr}`,
      );
    }
    await delay(50);
  }
  await delay(500);
}

/** 入口：跑方法序回放；红 = 方法序不符 / scope 序列超出已登记偏差 / DB 行数不符。 */
export async function runLifecycleReplay(): Promise<void> {
  const fixturePath = join(FIXTURE_DIR, "session-lifecycle-events.json");
  const fixture = JSON.parse(await readFile(fixturePath, "utf8")) as {
    expect: {
      requests: { method: string }[];
      reverseRequests: { method: string; params: { scope?: string } }[];
    };
    input: { prompts: string[]; script: unknown };
  };
  const expectedMethods = fixture.expect.requests.map((request) => request.method);
  // 二进制缺失（且 cargo build 失败）→ 显式红（Task 0 的「binary 不存在即红」纪律）。
  const binary = ensureAgentServerBinary();
  process.stdout.write(`agent-server=${binary}\nfixture=${fixturePath}\n`);

  const dataRoot = await mkdtemp(join(tmpdir(), "mode-c1-lifecycle-replay-"));
  const cwd = await mkdtemp(join(tmpdir(), "mode-c1-lifecycle-replay-ws-"));
  try {
    await seedWorkspace(cwd, SEED);
    // 基线库由 startAgentServer 内建（不要在这里重复建，表已存在会报错）。
    // 夹具剧本写成临时夹具（scripted provider 只读 `script` 字段）。
    const scriptFixture = join(dataRoot, "session-lifecycle-replay-fixture.json");
    await mkdir(dataRoot, { recursive: true });
    await writeFile(
      scriptFixture,
      `${JSON.stringify({ case: "session-lifecycle-replay", script: fixture.input.script }, null, 2)}\n`,
      "utf8",
    );

    const session = await startAgentServer({ dataRoot, cwd, fixturePath: scriptFixture });
    try {
      // 1. 方法序：create/subscribe 由 harness 发出（与夹具 #1/#2 同参形状）；
      //    其余按夹具顺序逐个发出并断言成功。
      const issued: string[] = ["session/create", "session/subscribe"];
      for (const prompt of fixture.input.prompts) {
        await session.send(prompt);
        issued.push("session/send");
      }
      await waitForReplayTerminals(session, fixture.input.prompts.length);
      const tailMethods = [
        "session/close",
        "session/resume",
        "session/read",
        "session/messages",
        "session/list",
      ];
      for (const method of tailMethods) {
        const params =
          method === "session/list"
            ? { includeArchived: true }
            : { sessionId: session.sessionId };
        await session.request(method, params);
        issued.push(method);
        await delay(300);
      }
      assert.deepEqual(issued, expectedMethods, "宿主方法序与 oracle 夹具不一致");

      // 2. 反向请求 scope 序列：C1 = [M, U, M]（resume 的 user-execution 延到首个
      //    send——本用例 resume 后没有 send，因此没有第 4 条）；oracle = [M, U, M, U]。
      //    放行差异 = 已登记偏差（§C1.4-1），其余逐字相等。
      const actualScopes = session.reverseRequests.map((request) => {
        const params = request.params as { scope?: string };
        return params.scope ?? "(none)";
      });
      const expectedScopes = fixture.expect.reverseRequests.map(
        (request) => request.params.scope ?? "(none)",
      );
      // **按位置**比较，不排序：前三条必须逐字一致，唯一放行的是 oracle 尾部
      // resume 路径的 `user-execution`（C1 延到首个 send，而本用例 resume 后没有 send）。
      // 旧实现把两侧排序后比较，会掩盖顺序漂移。
      const registeredGap = "user-execution";
      const expectedWithGap = [...expectedScopes];
      const lastUserExecution = expectedWithGap.lastIndexOf(registeredGap);
      if (lastUserExecution >= 0) {
        expectedWithGap.splice(lastUserExecution, 1);
      }
      if (JSON.stringify(actualScopes) !== JSON.stringify(expectedWithGap)) {
        throw new Error(
          `反向请求 scope 序列超出已登记偏差：实际 [${actualScopes.join(", ")}]，` +
            `oracle [${expectedScopes.join(", ")}]（已登记放行：resume 路径尾部的 ${registeredGap}）`,
        );
      }
      process.stdout.write(
        `已登记偏差放行：resume 路径的 user-execution 延到首个 send ` +
          `（实际 [${actualScopes.join(", ")}] / oracle [${expectedScopes.join(", ")}]）\n`,
      );

      // 3. DB 行数：oracle 观察减去已登记缺口（timeline 消息+part、shell entry）。
      const context: PathContext = { dataRoot, cwd, tmpDir: tmpdir() };
      const db = readDb(join(dataRoot, ".mode", "cli", "db", "db.sqlite"), context);
      const tables = db.tables as JsonObject;
      const actualRows: Record<string, number> = Object.fromEntries(
        Object.entries(tables).map(([name, meta]) => [
          name,
          (meta as { rowCount: number }).rowCount,
        ]),
      );
      // oracle 观察值（contract §3）：session 1、message 6、part 12、session_input 0、
      // session_entry 3。C1 的已登记缺口：合成 timeline 消息行（message −1）、
      // 其 timeline part（part −1）、bash_shell_selection entry（session_entry −1）。
      const oracleRows = { session: 1, message: 6, part: 12, session_input: 0, session_entry: 3 };
      const registeredGaps: Record<string, number> = { message: 1, part: 1, session_entry: 1 };
      const problems: string[] = [];
      for (const [table, oracleCount] of Object.entries(oracleRows)) {
        const actualCount = actualRows[table];
        const gap = registeredGaps[table] ?? 0;
        if (actualCount !== oracleCount - gap) {
          problems.push(`${table}: 实际 ${actualCount}，oracle ${oracleCount}（已登记缺口 ${gap}）`);
        }
      }
      if (problems.length > 0) {
        throw new Error(`DB 行数与 oracle 观察不符：${problems.join("; ")}`);
      }
      process.stdout.write(
        `已登记缺口放行：message/part 各 −1（合成 timeline 行）、session_entry −1（bash_shell_selection）\n`,
      );
      process.stdout.write(
        `${JSON.stringify(
          {
            agentServer: binary,
            methods: issued,
            reverseScopes: actualScopes,
            dbRows: actualRows,
            repoRoot: REPO_ROOT,
          },
          null,
          2,
        )}\n方法序回放通过（差异均为已登记项）。\n`,
      );
    } finally {
      await session.stop();
    }
  } finally {
    if (!process.argv.includes("--keep")) {
      await rm(dataRoot, { recursive: true, force: true });
      await rm(cwd, { recursive: true, force: true });
    }
  }
}
