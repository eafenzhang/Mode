/**
 * fpk 生命周期脚本（scripts/fnos-fpk/cmd/main）的 start/stop/status 与升级回滚回归。
 *
 * 这里不依赖 fnpack 与真实 Linux 载荷：用合成 app/ 目录 + 系统 node 包装器执行同一份
 * 生命周期脚本，验证「PID 唯一所有者、失败升级按 .update/backup 回滚」两条契约
 * （docs/specs/fnos-fpk-packaging.md、docs/specs/fnos-feature-parity-and-update.md）。
 *
 * 默认只在 Linux 跑（POSIX 进程/信号语义）；Windows 上如需在 Git Bash 本地验证，
 * 设 MODE_FPK_LIFECYCLE_TEST_WINDOWS=1。
 */
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmod, cp, mkdtemp, mkdir, readFile, rename, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";import { fileURLToPath } from "node:url";
import test from "node:test";

const MAIN_SCRIPT = fileURLToPath(
  new URL("../../../scripts/fnos-fpk/cmd/main", import.meta.url),
);

const SKIP_REASON =
  process.platform === "win32" && process.env["MODE_FPK_LIFECYCLE_TEST_WINDOWS"] !== "1"
    ? "requires POSIX process semantics（MODE_FPK_LIFECYCLE_TEST_WINDOWS=1 可强制在 Git Bash 验证）"
    : false;

function runMain(script: string, action: string, env: NodeJS.ProcessEnv) {
  const result = spawnSync("bash", [script, action], {
    env,
    encoding: "utf8",
    timeout: 30_000,
  });
  return {
    status: result.status,
    stdout: result.stdout ?? "",
    stderr: result.stderr ?? "",
  };
}

function isAlive(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

async function writePayload(appDir: string, marker: string): Promise<void> {
  await mkdir(join(appDir, "bin"), { recursive: true });
  await writeFile(
    join(appDir, "bin", "mode.mjs"),
    [
      `// ${marker}`,
      'process.on("SIGTERM", () => process.exit(0));',
      "setInterval(() => {}, 1000);",
      "",
    ].join("\n"),
    "utf8",
  );
  // ensure_node_binary 只要求可执行；用系统 node 代替 Linux 运行时（本测试只验证脚本编排）。
  await mkdir(join(appDir, "runtime", "bin"), { recursive: true });
  const nodeWrapper = join(appDir, "runtime", "bin", "node");
  await writeFile(nodeWrapper, '#!/bin/bash\nexec node "$@"\n', "utf8");
  await chmod(nodeWrapper, 0o755);
  await mkdir(join(appDir, "ui"), { recursive: true });
  await writeFile(
    join(appDir, "ui", "config"),
    JSON.stringify({ url: "/app/mode/?token=__MODE_TOKEN__" }),
    "utf8",
  );
}

test("cmd/main：start/status/stop 与失败升级回滚", { skip: SKIP_REASON }, async () => {
  const root = await mkdtemp(join(tmpdir(), "mode-fpk-main-"));
  const appDir = join(root, "app");
  const env: NodeJS.ProcessEnv = {
    ...process.env,
    TRIM_APPDEST: appDir,
    TRIM_PKGVAR: join(root, "var"),
    TRIM_PKGETC: join(root, "etc"),
    TRIM_PKGHOME: join(root, "home"),
    TRIM_SERVICE_PORT: "39999",
    TRIM_TEMP_LOGFILE: join(root, "notice.txt"),
    TRIM_USERNAME: "",
    TRIM_DATA_ACCESSIBLE_PATHS: "",
  };
  const infoLog = join(root, "var", "info.log");

  try {
    await mkdir(join(root, "var"), { recursive: true });
    await mkdir(join(root, "etc"), { recursive: true });
    await mkdir(join(root, "home"), { recursive: true });
    await writePayload(appDir, "old");
    await writeFile(join(root, "etc", "workspace"), join(root, "workspace"), "utf8");

    assert.equal(runMain(MAIN_SCRIPT, "status", env).status, 3, "未启动时 status 应为 3");
    const started = runMain(MAIN_SCRIPT, "start", env);
    assert.equal(started.status, 0, `start 失败：${started.stdout}${started.stderr}`);
    assert.equal(runMain(MAIN_SCRIPT, "status", env).status, 0, "start 后 status 应为 0");

    const pid = Number((await readFile(join(root, "var", "app.pid"), "utf8")).trim());
    // Git Bash 写的是 msys PID，Node 的 process.kill 只认 Windows PID；本地强制跑时以 status 为准。
    if (process.platform !== "win32") {
      assert.ok(pid > 0 && isAlive(pid), "start 后 PID 文件指向存活进程");
    }
    assert.match(await readFile(infoLog, "utf8"), /started pid=/u);

    assert.equal(runMain(MAIN_SCRIPT, "stop", env).status, 0);
    assert.equal(runMain(MAIN_SCRIPT, "status", env).status, 3, "stop 后 status 应为 3");
    if (process.platform !== "win32") {
      assert.equal(isAlive(pid), false, "stop 后进程必须退出");
    }

    // 失败升级：新载荷秒退 + in-progress 标记 + 完整 backup → 脚本应回滚并启动旧载荷。
    const backupTmp = join(root, "backup-tmp");
    await cp(appDir, backupTmp, { recursive: true });
    await mkdir(join(appDir, ".update"), { recursive: true });
    await rename(backupTmp, join(appDir, ".update", "backup"));
    await writeFile(join(appDir, ".update", "in-progress"), "update", "utf8");
    await writeFile(join(appDir, "bin", "mode.mjs"), "process.exit(1);\n", "utf8");

    const recovered = runMain(MAIN_SCRIPT, "start", env);
    assert.equal(recovered.status, 0, `回滚后启动失败：${recovered.stdout}${recovered.stderr}`);
    const logAfterRecovery = await readFile(infoLog, "utf8");
    assert.match(logAfterRecovery, /found interrupted update marker/u);
    assert.match(logAfterRecovery, /rolling back staged update/u);
    assert.match(
      await readFile(join(appDir, "bin", "mode.mjs"), "utf8"),
      /\/\/ old/u,
      "回滚必须恢复旧载荷文件",
    );
    await assert.rejects(stat(join(appDir, ".update")), "回滚完成后 .update 应被清理");
    assert.equal(runMain(MAIN_SCRIPT, "status", env).status, 0, "回滚后进程应运行");
  } finally {
    runMain(MAIN_SCRIPT, "stop", env);
    await rm(root, { recursive: true, force: true });
  }
});
