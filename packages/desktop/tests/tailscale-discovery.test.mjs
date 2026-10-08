import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

// Tailscale 单播发现回归（spec: docs/specs/lan-discovery.md）：
// 广播覆盖二层网段，tailnet 的 100.x 网段不透广播——discoverLanPeers 必须能把
// `tailscale status --json` 解析出的 IP 逐个单播探测并合并结果。
// 失效模式（未实现时）：只广播 → 注入的 127.0.0.1 应答器收不到探针 → 断言失败。
test("Tailscale 单播发现：解析 status JSON + 向 tailnet IP 单播探针并合并应答", async () => {
  const fixture = fileURLToPath(new URL("./fixtures/tailscale-discovery.ts", import.meta.url));
  const repoRoot = fileURLToPath(new URL("../../../", import.meta.url));
  const child = spawn(process.execPath, ["--import", "tsx", fixture], {
    cwd: repoRoot,
    env: process.env,
    stdio: ["ignore", "pipe", "pipe"],
  });

  let output = "";
  child.stdout.on("data", (chunk) => {
    output += String(chunk);
  });
  child.stderr.on("data", (chunk) => {
    output += String(chunk);
  });

  const exitCode = await new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      child.kill();
      reject(new Error(`夹具进程 30s 未退出\n${output}`));
    }, 30_000);
    child.once("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.once("close", (code) => {
      clearTimeout(timer);
      resolve(code);
    });
  });

  if (output.includes("TAILSCALE_DISCOVERY_SKIP")) {
    // 45879 被真实 LAN 服务占用（开发机常见）：单播路径由 CI/空闲机器覆盖。
    console.log("45879 被占用，跳过本机应答器场景");
    return;
  }
  assert.equal(exitCode, 0, `夹具失败（单播发现未生效）：\n${output}`);
  assert.match(output, /TAILSCALE_DISCOVERY_OK/, `夹具未完成断言：\n${output}`);
});
