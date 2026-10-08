import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

// 配对 label 回归：host 侧 pairLanPeer 未显式传 label 时必须兜底本机 hostname——
// 对端设置页「配对我的设备」按 label 显示设备名（此前向导传 navigator.platform，
// Windows 上就是 "Win32"，对端列表显示 win32）。
test("pairLanPeer label：缺省兜底 hostname，显式 label 优先", async () => {
  const fixture = fileURLToPath(new URL("./fixtures/pair-label.ts", import.meta.url));
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

  assert.equal(exitCode, 0, `夹具失败（label 未兜底 hostname 或显式 label 失效）：\n${output}`);
  assert.match(output, /PAIR_LABEL_OK/, `夹具未完成断言：\n${output}`);
});
