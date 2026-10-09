import test from "node:test";
import assert from "node:assert/strict";
import { fork, spawnSync } from "node:child_process";
import { writeFileSync, mkdtempSync } from "node:fs";
import net from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { callBrokerMethod, mintBrokerSocketPath } from "../broker.js";
import { HELPER_PROTOCOL_VERSION } from "../broker-server.js";
import { createHelperServer } from "../helper/server.mjs";

const entry = fileURLToPath(new URL("../helper/entry.mjs", import.meta.url));
const CONTROL_PROTOCOL = "mode-cua-windows-dev/v1";

// fake addon：满足 Task 8 的 backend 契约，不依赖真 UIA。
// health 故意返回 pid:42 —— controller 裁决 health 由 backend 合成（= helper 自身 pid），
// 该导出必须被忽略；若 backend 读了 addon.health，roundtrip 断言会当场失败。
const fakeAddonPath = join(mkdtempSync(join(tmpdir(), "cua-fake-")), "fake-addon.cjs");
writeFileSync(
  fakeAddonPath,
  `
  module.exports = {
    version: () => "0.0.0-test",
    health: () => ({ bundleId: null, pid: 42 }),
    list_apps: () => [{ pid: 1, name: "fake.exe", bundle_id: null, active: false }],
    list_windows: () => [], observe: () => {}, capture: () => {},
    perform: () => ({ dispatched: "dispatched" }), launch_app: () => {}, screen_probe: () => ({ locked: false }),
  };
`,
);

// teardown：kill + 等待 exit，断言失败路径同样收尸，绝不泄漏子进程。
function reap(child) {
  if (child.exitCode !== null || child.signalCode !== null) return Promise.resolve();
  return new Promise((resolve) => {
    // 兜底：极端情况下再杀一次并放行，测试不因收尸挂死（TerminateProcess 对自有子进程可靠）。
    const hardStop = setTimeout(() => {
      child.kill();
      resolve();
    }, 5_000);
    child.once("exit", () => {
      clearTimeout(hardStop);
      resolve();
    });
    if (!child.kill()) {
      clearTimeout(hardStop);
      resolve();
    }
  });
}

// env 为完整子进程环境（调用方自行基于 process.env 构造，delete 语义不被合并覆盖）。
function forkEntry(args, env) {
  return fork(entry, args, {
    stdio: ["ignore", "pipe", "pipe", "ipc"],
    env,
  });
}

function awaitReady(child, timeoutMs = 5_000) {
  return new Promise((resolve, reject) => {
    const messages = [];
    const timer = setTimeout(() => reject(new Error(`no ready in ${timeoutMs}ms`)), timeoutMs);
    child.on("message", (m) => {
      messages.push(m);
      if (m?.type === "ready") {
        clearTimeout(timer);
        resolve({ ready: m, messages });
      }
      if (m?.type === "error") {
        clearTimeout(timer);
        reject(new Error(JSON.stringify(m.error)));
      }
    });
  });
}

function awaitExit(child, timeoutMs = 5_000) {
  if (child.exitCode !== null || child.signalCode !== null) {
    return Promise.resolve(child.exitCode);
  }
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`no exit in ${timeoutMs}ms`)), timeoutMs);
    child.once("exit", (code) => {
      clearTimeout(timer);
      resolve(code);
    });
  });
}

test(
  "helper handshakes and serves health over the pipe",
  { skip: process.platform !== "win32" },
  async () => {
    const socketPath = mintBrokerSocketPath();
    const child = forkEntry(["--socket", socketPath, "--parent-pid", String(process.pid)], {
      ...process.env,
      MODE_CUA_HELPER_ADDON: fakeAddonPath,
    });
    try {
      const { ready, messages } = await awaitReady(child);
      assert.equal(ready.protocol, CONTROL_PROTOCOL);
      assert.equal(ready.socketPath, socketPath);
      assert.equal(ready.pid, child.pid);
      // 握手顺序：transport_ready（管道已绑定）必须先于 ready，字段与 ready 同契约
      // （parseReadyMessage 对二者都强制 socketPath:string + pid:正整数）。
      const transport = messages.find((m) => m?.type === "transport_ready");
      assert.ok(transport, "transport_ready missing");
      assert.equal(transport.protocol, CONTROL_PROTOCOL);
      assert.equal(transport.socketPath, socketPath);
      assert.equal(transport.pid, child.pid);
      assert.ok(
        messages.indexOf(transport) < messages.indexOf(ready),
        "transport_ready must precede ready",
      );
      const health = await callBrokerMethod({ socketPath, method: "health", timeoutMs: 2_000 });
      assert.equal(typeof health.pid, "number");
      // health 由 backend 合成 = helper 子进程 pid；addon.health(pid:42) 必须被忽略。
      assert.equal(health.pid, child.pid);
      const apps = await callBrokerMethod({ socketPath, method: "list_apps", timeoutMs: 2_000 });
      assert.equal(apps[0].name, "fake.exe");
    } finally {
      await reap(child);
    }
  },
);

test(
  "startup failure sends error handshake and exits non-zero",
  { skip: process.platform !== "win32" },
  async () => {
    const child = forkEntry(
      ["--socket", mintBrokerSocketPath(), "--parent-pid", String(process.pid)],
      {
        ...process.env,
        MODE_CUA_HELPER_ADDON: join(tmpdir(), "cua-missing-addon.cjs"),
      },
    );
    try {
      const exited = new Promise((resolve) => {
        child.once("exit", (code, signal) => resolve({ code, signal }));
      });
      const error = await new Promise((resolve, reject) => {
        const t = setTimeout(() => reject(new Error("no error handshake in 5s")), 5_000);
        child.on("message", (m) => {
          if (m?.type === "error") {
            clearTimeout(t);
            resolve(m);
          }
        });
      });
      // parseReadyMessage 的 error 分支只认非空顶层 message:string；error 对象是附加诊断面。
      assert.equal(error.protocol, CONTROL_PROTOCOL);
      assert.equal(typeof error.message, "string");
      assert.ok(error.message.length > 0);
      assert.equal(error.error.code, "internal");
      const { code } = await exited;
      assert.notEqual(code, 0);
    } finally {
      await reap(child);
    }
  },
);

test(
  "missing addon env reports error handshake and exits non-zero",
  { skip: process.platform !== "win32" },
  async () => {
    const env = { ...process.env };
    delete env.MODE_CUA_HELPER_ADDON;
    const child = forkEntry(["--socket", mintBrokerSocketPath()], env);
    try {
      const exited = awaitExit(child);
      const error = await new Promise((resolve, reject) => {
        const t = setTimeout(() => reject(new Error("no error handshake in 5s")), 5_000);
        child.on("message", (m) => {
          if (m?.type === "error") {
            clearTimeout(t);
            resolve(m);
          }
        });
      });
      assert.equal(error.protocol, CONTROL_PROTOCOL);
      assert.ok(error.message.includes("MODE_CUA_HELPER_ADDON"));
      assert.equal(error.error.code, "internal");
      assert.notEqual(await exited, 0);
    } finally {
      await reap(child);
    }
  },
);

test(
  "host shutdown message closes the helper with exit 0",
  { skip: process.platform !== "win32" },
  async () => {
    const socketPath = mintBrokerSocketPath();
    const child = forkEntry(["--socket", socketPath, "--parent-pid", String(process.pid)], {
      ...process.env,
      MODE_CUA_HELPER_ADDON: fakeAddonPath,
    });
    try {
      await awaitReady(child);
      const exited = awaitExit(child);
      // host 的优雅关闭通道：WindowsCuaChildLifecycle.sendShutdown 的原样消息。
      child.send({ protocol: CONTROL_PROTOCOL, type: "shutdown" });
      assert.equal(await exited, 0);
    } finally {
      await reap(child);
    }
  },
);

test(
  "parent-pid watchdog exits 0 when the parent is gone",
  { skip: process.platform !== "win32" },
  async () => {
    // 先造一个确定已死的 pid（spawnSync 返回时子进程必然已退出；pid 回收窗口极小）。
    const probe = spawnSync(process.execPath, ["-e", ""], { stdio: "ignore" });
    assert.ok(probe.pid > 0);
    const child = forkEntry(
      ["--socket", mintBrokerSocketPath(), "--parent-pid", String(probe.pid)],
      { ...process.env, MODE_CUA_HELPER_ADDON: fakeAddonPath },
    );
    try {
      await awaitReady(child);
      // 看门狗 2s 一跳，首跳 ESRCH → exit(0)（孤儿回收兜底）。
      assert.equal(await awaitExit(child, 4_000), 0);
    } finally {
      await reap(child);
    }
  },
);

test("server buffers split chunks and answers sequential requests on one connection", async () => {
  const socketPath = mintBrokerSocketPath();
  const server = createHelperServer({
    list_apps: () => [{ pid: 1, name: "fake.exe", bundle_id: null, active: false }],
  });
  await new Promise((resolve) => server.listen(socketPath, resolve));
  try {
    const responses = await new Promise((resolve, reject) => {
      const sock = net.connect(socketPath);
      const collected = [];
      let buffer = "";
      const timer = setTimeout(
        () => reject(new Error(`only ${collected.length}/2 responses in 2s`)),
        2_000,
      );
      sock.on("error", (error) => {
        clearTimeout(timer);
        reject(error);
      });
      sock.on("connect", () => {
        // 首个请求被人为分片：半行先发、余量后发 —— 行缓冲必须拼回完整请求。
        sock.write('{"id":"1","method":"hea');
        setTimeout(() => {
          sock.write('lth","params":{}}\n{"id":"2","method":"list_apps","params":{}}\n');
        }, 5);
      });
      sock.on("data", (chunk) => {
        buffer += chunk;
        let newlineAt;
        while ((newlineAt = buffer.indexOf("\n")) !== -1) {
          collected.push(JSON.parse(buffer.slice(0, newlineAt)));
          buffer = buffer.slice(newlineAt + 1);
        }
        if (collected.length === 2) {
          clearTimeout(timer);
          sock.destroy();
          resolve(collected);
        }
      });
    });
    // 同连接两条请求按行序作答（响应不回显 id，顺序即契约）。
    // protocolVersion 为 Task 8 裁决（progress.md:88）：backend 合成 health 携带协议版本，
    // Plan B runtime 首调比对不符 → version_mismatch。
    assert.deepEqual(responses[0], {
      ok: true,
      result: { bundleId: null, pid: process.pid, protocolVersion: HELPER_PROTOCOL_VERSION },
    });
    assert.deepEqual(responses[1], {
      ok: true,
      result: [{ pid: 1, name: "fake.exe", bundle_id: null, active: false }],
    });
  } finally {
    await new Promise((resolve) => server.close(resolve));
  }
});
