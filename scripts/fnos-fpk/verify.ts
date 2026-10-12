// fnOS fpk 验收：结构断言、令牌行为（对 fpk 内 payload 实测）。
// 契约与验收场景见 docs/specs/fnos-fpk-packaging.md。

import { spawn } from "node:child_process";
import { once } from "node:events";
import { createRequire } from "node:module";
import { mkdir, open, readFile, rm } from "node:fs/promises";
import { join } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { pathToFileURL } from "node:url";
import assert from "node:assert/strict";

import { checkElf, pathExists, run, tarArgs, tarLines, workDir } from "./build-lib.ts";

async function assertExecutable(archive: string, member: string): Promise<void> {
  const lines = await tarLines(archive, { verbose: true });
  const line = lines.find((item) => item.split(/\s+/u).at(-1) === member);
  if (!line) {
    throw new Error(`归档缺少成员：${member}`);
  }
  if (!line.startsWith("-rwx")) {
    throw new Error(`${member} 缺少执行位（Windows 打包后处理失效）：${line}`);
  }
}

async function pngSize(file: string): Promise<{ width: number; height: number }> {
  const handle = await open(file, "r");
  try {
    const buffer = Buffer.alloc(24);
    await handle.read(buffer, 0, 24, 0);
    const magic = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
    assert.ok(buffer.subarray(0, 8).equals(magic), `${file} 不是 PNG`);
    return { width: buffer.readUInt32BE(16), height: buffer.readUInt32BE(20) };
  } finally {
    await handle.close();
  }
}

export async function until<T>(
  check: () => T | null | Promise<T | null>,
  label: string,
  diagnostic: () => string,
  timeoutMs = 30000,
): Promise<T> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await check();
    if (value) {
      return value;
    }
    if (Date.now() > deadline) {
      throw new Error(`${label} 超时：${diagnostic()}`);
    }
    await delay(100);
  }
}

export async function verifyStructure({
  fpkFile,
  verifyDir,
  version,
  port,
}: {
  fpkFile: string;
  verifyDir: string;
  version: string;
  port: number;
}): Promise<void> {
  const lines = await tarLines(fpkFile, { verbose: true });
  const names = lines.map((item) => item.split(/\s+/u).at(-1));
  for (const required of [
    "manifest",
    "app.tgz",
    "cmd/main",
    "cmd/install_callback",
    "cmd/upgrade_callback",
    "cmd/lib.sh",
    "config/privilege",
    "config/resource",
    "ICON.PNG",
    "ICON_256.PNG",
  ]) {
    assert.ok(names.includes(required), `fpk 缺少成员：${required}`);
  }
  await assertExecutable(fpkFile, "cmd/main");

  const manifest = await readFile(join(workDir, "repack", "manifest"), "utf8");
  const escapedVersion = version.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&");
  assert.match(manifest, new RegExp(`^version\\s*=\\s*${escapedVersion}\\s*$`, "mu"));
  assert.match(manifest, /^platform\s*=\s*x86\s*$/mu);
  assert.match(manifest, new RegExp(`^service_port\\s*=\\s*${port}\\s*$`, "mu"));
  assert.match(manifest, /^desktop_uidir\s*=\s*ui\s*$/mu);

  await rm(verifyDir, { recursive: true, force: true });
  await mkdir(verifyDir, { recursive: true });
  run("tar", tarArgs(["-xzf", join(workDir, "repack", "app.tgz"), "-C", verifyDir]));

  for (const required of [
    "bin/mode.mjs",
    "server/entry-http.js",
    "web/index.html",
    "agent/mode.cjs",
    "node_modules/node-pty",
    "runtime/bin/node",
    "runtime/LICENSE",
    "ui/config",
    "ui/images/icon_64.png",
    "ui/images/icon_256.png",
  ]) {
    assert.ok(await pathExists(join(verifyDir, required)), `payload 缺少：${required}`);
  }
  const nodeSize = await checkElf(join(verifyDir, "runtime/bin/node"), "payload 捆绑 Node");
  assert.ok(nodeSize > 50 * 1024 * 1024, "payload 捆绑 Node 体积异常");

  const uiConfig = JSON.parse(await readFile(join(verifyDir, "ui", "config"), "utf8")) as {
    ".url": Record<string, { url: string; port: string; type: string }>;
  };
  const entry = uiConfig[".url"]?.["mode.Application"];
  assert.ok(entry, "ui 入口 mode.Application 缺失");
  assert.equal(entry.type, "iframe", "ui 入口应为 iframe（飞牛桌面内嵌打开）");
  assert.match(entry.url, /token=__MODE_TOKEN__/u, "ui 入口应携带令牌占位符");
  assert.equal(String(entry.port), String(port));

  const icon64 = await pngSize(join(verifyDir, "ui", "images", "icon_64.png"));
  assert.deepEqual(icon64, { width: 64, height: 64 });
  const icon256 = await pngSize(join(verifyDir, "ui", "images", "icon_256.png"));
  assert.deepEqual(icon256, { width: 256, height: 256 });
}

// 令牌行为验收：401 / 入口带令牌 200 + Cookie / API 与 WS 携带 Cookie 可用 / 退出。
export async function verifyTokenFlow(payloadDir: string): Promise<void> {
  const runner = join(payloadDir, "bin", "mode.mjs");
  const workspace = join(workDir, "token-workspace");
  const dataDir = join(workDir, "token-data");
  await rm(workspace, { recursive: true, force: true });
  await rm(dataDir, { recursive: true, force: true });
  await mkdir(workspace, { recursive: true });
  await mkdir(dataDir, { recursive: true });

  const token = "fnos-token-flow-check";
  const child = spawn(
    process.execPath,
    [runner, "--web", "--host", "0.0.0.0", "--token", token, "--workspace", workspace, "--no-open"],
    {
      cwd: workspace,
      env: { ...process.env, MODE_DATA_BASE_DIR: dataDir },
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  let output = "";
  child.stdout.on("data", (chunk: Buffer) => {
    output += chunk;
  });
  child.stderr.on("data", (chunk: Buffer) => {
    output += chunk;
  });

  try {
    const base = await until(
      () => output.match(/Local:\s+(http:\/\/127\.0\.0\.1:\d+\/)/u)?.[1] ?? null,
      "Web 启动",
      () => output,
    );

    // runner 打印 Local URL 先于服务真正监听，首个请求可能 ECONNREFUSED；重试到拿到任意 HTTP 响应。
    const firstResponse = await until(
      async () => {
        try {
          return await fetch(new URL("api/server-info", base), {
            signal: AbortSignal.timeout(1000),
          });
        } catch {
          return null;
        }
      },
      "服务就绪",
      () => output,
    );
    assert.equal(firstResponse.status, 401, "未带令牌的 /api 应 401");

    const landing = await fetch(new URL(`/?token=${token}`, base));
    assert.equal(landing.status, 200, "带令牌的入口页应 200");
    const html = await landing.text();
    assert.match(html, /<html/iu, "入口页应为 HTML");
    const setCookie = landing.headers.get("set-cookie") ?? "";
    const cookie = setCookie.split(";")[0]?.trim();
    assert.ok(cookie?.includes("="), `入口页应下发令牌 Cookie（收到：${setCookie || "空"}）`);

    const authenticated = await fetch(new URL("api/server-info", base), { headers: { cookie } });
    assert.equal(authenticated.status, 200, "携带 Cookie 的 /api 应 200");
    const info = (await authenticated.json()) as { workspaces?: Array<{ path: string }> };
    assert.equal(info.workspaces?.[0]?.path, workspace, "workspaces[0] 应为传入工作区");

    const require = createRequire(join(payloadDir, "package.json"));
    const { default: WebSocket } = (await import(
      pathToFileURL(require.resolve("ws")).href
    )) as { default: typeof globalThis.WebSocket };
    const socket = new WebSocket(new URL("ws", base.replace("http:", "ws:")), {
      headers: { cookie },
    } as unknown as undefined);
    const opened = await Promise.race([
      once(socket, "open").then(() => true),
      once(socket, "error").then(() => false),
      delay(10000).then(() => null),
    ]);
    assert.equal(opened, true, "携带令牌的 WebSocket 应可建立");
    socket.close();

    const exited = once(child, "exit");
    child.kill("SIGTERM");
    const [code] = await Promise.race([
      exited,
      delay(15000).then((): [number | null] => {
        throw new Error("服务进程未按预期退出");
      }),
    ]);
    if (process.platform === "win32") {
      // Windows 没有 POSIX 信号：kill 走 TerminateProcess，退出码非 0 属平台语义（冒烟脚本同因只在 POSIX 跑）。
      console.log(`[fnos] 令牌行为检查通过（win32 退出码 ${code}，POSIX 断言优雅退出）`);
    } else {
      assert.equal(code, 0, "SIGTERM 应优雅退出（exit 0）");
    }
  } catch (error) {
    console.error(`[fnos] 令牌行为检查失败；子进程输出：\n${output}`);
    throw error;
  } finally {
    if (child.exitCode === null && child.signalCode === null) {
      child.kill("SIGKILL");
    }
  }
}
