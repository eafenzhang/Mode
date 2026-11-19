// `mode eval` 命令（spec §3）：run / replay 两个子命令；agent-server 命令解析有明确优先级。
import { existsSync } from "node:fs";
import { writeFile, mkdir } from "node:fs/promises";
import { join, resolve } from "node:path";

import { createAgentServerSessionFactory, type AgentServerLaunch } from "./agent-server.js";
import { diffRuns, formatRunDiffText } from "./diff.js";
import { createRedactor } from "./redact.js";
import { formatReportText, stripTiming } from "./report.js";
import { replayTrajectory, runSuite } from "./runner.js";
import { loadSuite } from "./suite.js";

export type EvalCommandContext = {
  stdout: NodeJS.WritableStream;
  stderr: NodeJS.WritableStream;
};

export type EvalCommandOptions = {
  json?: boolean;
  evalOut?: string;
  evalTimeout?: string;
  evalAgentServer?: string;
  env?: NodeJS.ProcessEnv;
  cwd?: string;
};

export class EvalConfigError extends Error {
  constructor(
    message: string,
    readonly code: string,
  ) {
    super(message);
    this.name = "EvalConfigError";
  }
}

const USAGE = `Usage:
  mode eval run <suite.json|dir> [--eval-out <dir>] [--eval-timeout <ms>] [--eval-agent-server <path>]
  mode eval replay <trajectory.jsonl> [--eval-out <dir>] [--eval-agent-server <path>]
  mode eval diff <runDirA> <runDirB>`;

export async function runEvalCommand(
  ctx: EvalCommandContext,
  options: EvalCommandOptions,
  args: readonly string[],
): Promise<number> {
  const subcommand = args[0];
  if (subcommand !== "run" && subcommand !== "replay" && subcommand !== "diff") {
    ctx.stderr.write(`mode eval: unknown subcommand ${subcommand ?? "(missing)"}\n\n${USAGE}\n`);
    return 1;
  }
  if (subcommand === "diff") {
    const dirA = args[1];
    const dirB = args[2];
    if (!dirA || !dirB) {
      ctx.stderr.write(`mode eval diff: missing <runDirA> <runDirB>\n\n${USAGE}\n`);
      return 1;
    }
    const cwd = options.cwd ?? process.cwd();
    const diff = await diffRuns(resolve(cwd, dirA), resolve(cwd, dirB));
    if (options.json === true) {
      ctx.stdout.write(`${JSON.stringify(diff, null, 2)}\n`);
    } else {
      ctx.stdout.write(formatRunDiffText(diff));
    }
    return diff.identical ? 0 : 1;
  }
  const target = args[1];
  if (!target) {
    ctx.stderr.write(`mode eval ${subcommand}: missing <path>\n\n${USAGE}\n`);
    return 1;
  }

  const env = options.env ?? process.env;
  const cwd = options.cwd ?? process.cwd();
  // resolve(cwd, target) 接受绝对路径；join 会把绝对目标拼进 cwd。
  const outDir = resolve(cwd, options.evalOut ?? ".mode-eval");
  const defaultTimeoutMs = options.evalTimeout ? Number(options.evalTimeout) : undefined;
  if (options.evalTimeout !== undefined && (!Number.isFinite(defaultTimeoutMs) || (defaultTimeoutMs ?? 0) <= 0)) {
    ctx.stderr.write(`mode eval: --eval-timeout must be a positive number of ms\n`);
    return 1;
  }

  let launch: AgentServerLaunch;
  try {
    launch = resolveAgentServerLaunch({
      flag: options.evalAgentServer,
      env,
      cwd,
    });
  } catch (error) {
    if (error instanceof EvalConfigError) {
      ctx.stderr.write(`mode eval: ${error.code}: ${error.message}\n`);
      return 1;
    }
    throw error;
  }

  const redactor = createRedactor({
    homeDir: env.USERPROFILE ?? env.HOME,
    domains: (env.MODE_EVAL_REDACT_DOMAINS ?? "").split(","),
  });
  const sessionFactory = createAgentServerSessionFactory(launch, env);

  try {
    if (subcommand === "run") {
      const suitePath = resolve(cwd, target);
      const suite = await loadSuite(suitePath);
      const report = await runSuite({
        suite,
        suitePath,
        outDir,
        sessionFactory,
        redactor,
        defaultTimeoutMs,
      });
      await mkdir(outDir, { recursive: true });
      await writeFile(join(outDir, "report.json"), `${JSON.stringify(stripTiming(report), null, 2)}\n`, "utf8");
      if (options.json === true) {
        ctx.stdout.write(`${JSON.stringify(stripTiming(report), null, 2)}\n`);
      } else {
        ctx.stdout.write(formatReportText(report));
      }
      ctx.stdout.write(`artifacts: ${join(outDir, "trajectories")}, ${join(outDir, "report.json")}\n`);
      return report.totals.failed > 0 ? 1 : 0;
    }

    const trajectoryPath = resolve(cwd, target);
    const result = await replayTrajectory(trajectoryPath, {
      sessionFactory,
      redactor,
      defaultTimeoutMs,
    });
    const summary = {
      schemaVersion: 1,
      replay: trajectoryPath,
      case: result.case,
      status: result.status,
      mismatches: result.mismatches,
      error: result.error,
    };
    if (options.json === true) {
      ctx.stdout.write(`${JSON.stringify(summary, null, 2)}\n`);
    } else {
      ctx.stdout.write(buildSingleReport(result));
    }
    return result.status === "passed" ? 0 : 1;
  } catch (error) {
    const message = redactor.redactString(error instanceof Error ? error.message : String(error));
    ctx.stderr.write(`mode eval: ${message}\n`);
    return 1;
  }
}

function buildSingleReport(result: { case: string; status: string; durationMs: number; toolCalls: string[]; turns: number; mismatches: unknown[]; error?: string }): string {
  const lines = [`[${result.status}] ${result.case} (${result.durationMs}ms, tools: ${result.toolCalls.join(", ") || "none"})`];
  if (result.error) lines.push(`  - error: ${result.error}`);
  return `${lines.join("\n")}\n`;
}

export function resolveAgentServerLaunch(input: {
  flag?: string;
  env: NodeJS.ProcessEnv;
  cwd: string;
}): AgentServerLaunch {
  if (input.flag && input.flag.trim().length > 0) {
    return { command: input.flag.trim(), args: ["app-server", "--stdio"] };
  }
  const evalCommand = input.env.MODE_EVAL_AGENT_SERVER_COMMAND?.trim();
  if (evalCommand) {
    return { command: evalCommand, args: ["app-server", "--stdio"] };
  }
  const hostCommand = input.env.MODE_AGENT_SERVER_COMMAND?.trim();
  if (hostCommand) {
    const argsJson = input.env.MODE_AGENT_SERVER_ARGS_JSON?.trim();
    const parsed = argsJson ? (JSON.parse(argsJson) as unknown) : undefined;
    const args = Array.isArray(parsed)
      ? parsed.map((item) => String(item))
      : ["app-server", "--stdio"];
    return { command: hostCommand, args };
  }
  const found = findAgentServerBinary(input.cwd);
  if (found) {
    return { command: found, args: ["app-server", "--stdio"] };
  }
  throw new EvalConfigError(
    "no agent-server available. Set --eval-agent-server, MODE_EVAL_AGENT_SERVER_COMMAND, " +
      "MODE_AGENT_SERVER_COMMAND, or build one (cargo build -p agent-server).",
    "MODE_EVAL_AGENT_SERVER_UNAVAILABLE",
  );
}

/** 从 cwd 向上找 target/{debug,release}/agent-server(.exe)。 */
function findAgentServerBinary(from: string): string | undefined {
  const exeNames = process.platform === "win32" ? ["agent-server.exe", "agent-server"] : ["agent-server"];
  let dir = from;
  for (let depth = 0; depth < 32; depth += 1) {
    for (const profile of ["debug", "release"]) {
      for (const name of exeNames) {
        const candidate = join(dir, "target", profile, name);
        if (existsSync(candidate)) return candidate;
      }
    }
    const parent = join(dir, "..");
    if (parent === dir) return undefined;
    dir = parent;
  }
  return undefined;
}
