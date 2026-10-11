import assert from "node:assert/strict";
import net from "node:net";
import test from "node:test";
import { BrokerError, CuaHelperError, mintBrokerSocketPath } from "@mode/cua/broker";

// 远程桌面（LAN）Task 3b：helper 准入第二理由与被控注入端口。
// 准入谓词走 ../src/node.js（重装配模块，沿用 cua-helper-admission.test.ts 的动态导入形态）；
// 注入端口直接导入目标模块，socket 与 broker 客户端按依赖注入替换（目录既有 mock 风格）。
const { shouldCreateDefaultCuaProductHelper } = await import("../src/node.js");
const { isCuaExcludedRemoteWorkspaceIdentity } = await import("../src/cuaEnablement.js");
const { createCuaRawInputInjector } = await import("../src/cua-permission-broker/cuaRawInput.js");

// ── 准入矩阵（docs/specs/lan-remote-desktop.md「接口变更」第 4 条）──
// desktop-local + 无 remote identity + 未注入 resolver 的基准；两个准入理由各自独立可放行。
const ADMISSION_BASE = {
  serviceAuthorityMode: "desktop-local" as const,
  hasRemoteWorkspaceIdentity: false,
  hasInjectedResolver: false,
  hasBuiltInCuaPlugin: false,
};

test("准入第二理由：被控开关开 → 建（与 CUA 插件启用并列）", () => {
  assert.equal(
    shouldCreateDefaultCuaProductHelper({
      ...ADMISSION_BASE,
      isRemoteDesktopServerEnabled: () => true,
    }),
    true,
  );
});

test("准入：开关关 + 插件关 → 不建；缺省读取点即关闭（默认关）", () => {
  assert.equal(
    shouldCreateDefaultCuaProductHelper({
      ...ADMISSION_BASE,
      isRemoteDesktopServerEnabled: () => false,
    }),
    false,
  );
  assert.equal(shouldCreateDefaultCuaProductHelper({ ...ADMISSION_BASE }), false);
});

test("准入：插件已放行时不再读取开关（懒读取，不产生多余设置读）", () => {
  let reads = 0;
  assert.equal(
    shouldCreateDefaultCuaProductHelper({
      ...ADMISSION_BASE,
      hasBuiltInCuaPlugin: true,
      isRemoteDesktopServerEnabled: () => {
        reads += 1;
        return false;
      },
    }),
    true,
  );
  assert.equal(reads, 0);
});

test("准入：开关开但 authority 不满足（desktop-attached-remote / standalone-server）→ 不建", () => {
  assert.equal(
    shouldCreateDefaultCuaProductHelper({
      ...ADMISSION_BASE,
      serviceAuthorityMode: "desktop-attached-remote",
      isRemoteDesktopServerEnabled: () => true,
    }),
    false,
  );
  assert.equal(
    shouldCreateDefaultCuaProductHelper({
      ...ADMISSION_BASE,
      serviceAuthorityMode: "standalone-server",
      isRemoteDesktopServerEnabled: () => true,
    }),
    false,
  );
});

test("准入：开关开 + ssh/wsl/docker identity → 不建（identity 排除不因开关放宽）", () => {
  for (const identity of [
    "remote:ssh:host:22:alice:/tmp/ws",
    "remote:wsl:Ubuntu:/tmp/ws",
    "remote:docker:abc:/tmp/ws",
  ]) {
    assert.equal(
      shouldCreateDefaultCuaProductHelper({
        ...ADMISSION_BASE,
        hasRemoteWorkspaceIdentity: isCuaExcludedRemoteWorkspaceIdentity(identity),
        isRemoteDesktopServerEnabled: () => true,
      }),
      false,
      identity,
    );
  }
});

test("准入：desktop-local + lan identity 既有行为不回归（两条理由各自放行）", () => {
  const hasRemoteWorkspaceIdentity = isCuaExcludedRemoteWorkspaceIdentity(
    "remote:lan:192.168.1.5:3030:/tmp/ws",
  );
  assert.equal(hasRemoteWorkspaceIdentity, false);
  assert.equal(
    shouldCreateDefaultCuaProductHelper({
      ...ADMISSION_BASE,
      hasRemoteWorkspaceIdentity,
      hasBuiltInCuaPlugin: true,
    }),
    true,
  );
  assert.equal(
    shouldCreateDefaultCuaProductHelper({
      ...ADMISSION_BASE,
      hasRemoteWorkspaceIdentity,
      isRemoteDesktopServerEnabled: () => true,
    }),
    true,
  );
});

test("准入：开关开但已注入 resolver → 不建（不重复创建）", () => {
  assert.equal(
    shouldCreateDefaultCuaProductHelper({
      ...ADMISSION_BASE,
      hasInjectedResolver: true,
      isRemoteDesktopServerEnabled: () => true,
    }),
    false,
  );
});

// ── 注入端口（一次 raw_input 调用；不重试、不做前台/权限校验）──

test("注入成功：按 raw_input 线协议经既有通道调用并回传 dispatched", async () => {
  const socketPath = mintBrokerSocketPath();
  const requests: unknown[] = [];
  const server = net.createServer((socket) => {
    socket.on("error", () => {});
    let buffer = "";
    socket.on("data", (chunk) => {
      buffer += chunk.toString("utf8");
      const nl = buffer.indexOf("\n");
      if (nl < 0) return;
      requests.push(JSON.parse(buffer.slice(0, nl)));
      socket.write(`${JSON.stringify({ ok: true, result: { dispatched: 2 } })}\n`);
    });
  });
  await new Promise((resolve) => server.listen(socketPath, resolve));
  try {
    const injector = createCuaRawInputInjector({ resolveSocketPath: () => socketPath });
    const events = [
      { k: "move", x: 100, y: 200 },
      { k: "key", vk: 65, down: true },
    ];
    assert.deepEqual(await injector(events), { dispatched: 2 });
    // 线协议契约：单请求单行 JSON，method=raw_input，params.events 原样透传（形状校验归 Rust）。
    assert.deepEqual(requests, [{ id: "1", method: "raw_input", params: { events } }]);
  } finally {
    await new Promise((resolve) => server.close(resolve));
  }
});

test("注入不可达：socket 无监听者（helper 未运行/未启用）→ helper_unavailable", async () => {
  const socketPath = mintBrokerSocketPath(); // 只铸造路径，不建监听者
  const injector = createCuaRawInputInjector({ resolveSocketPath: () => socketPath });
  await assert.rejects(
    injector([{ k: "move", x: 0, y: 0 }]),
    (error) => error instanceof CuaHelperError && error.code === "helper_unavailable",
  );
});

test("注入不可达：解析点返回 null → helper_unavailable，且不发起调用", async () => {
  let calls = 0;
  const injector = createCuaRawInputInjector({
    resolveSocketPath: () => null,
    callBrokerMethod: async () => {
      calls += 1;
      return { dispatched: 1 };
    },
  });
  await assert.rejects(
    injector([{ k: "move", x: 0, y: 0 }]),
    (error) => error instanceof CuaHelperError && error.code === "helper_unavailable",
  );
  assert.equal(calls, 0);
});

test("注入不可达：连接层错误（stale_socket / timeout）收敛为 helper_unavailable", async () => {
  for (const code of ["stale_socket", "timeout", "broker_unavailable"]) {
    const injector = createCuaRawInputInjector({
      resolveSocketPath: () => "\\\\.\\pipe\\mode-cua-raw-input-test",
      callBrokerMethod: async () => {
        throw new BrokerError(`simulated ${code}`, { code });
      },
    });
    await assert.rejects(
      injector([{ k: "text", text: "a" }]),
      (error) => error instanceof CuaHelperError && error.code === "helper_unavailable",
      code,
    );
  }
});

test("注入失败：helper 明确应答的协议错误保留原码（invalid_request / method_not_found）", async () => {
  for (const code of ["invalid_request", "method_not_found"]) {
    const injector = createCuaRawInputInjector({
      resolveSocketPath: () => "\\\\.\\pipe\\mode-cua-raw-input-test",
      callBrokerMethod: async () => {
        throw new BrokerError(`helper rejected with ${code}`, { code });
      },
    });
    await assert.rejects(
      injector([{ k: "move", x: 1, y: 1 }]),
      (error) => error instanceof CuaHelperError && error.code === code,
      code,
    );
  }
});

test("参数门：非数组 events → invalid_request，且不发起调用", async () => {
  let calls = 0;
  const injector = createCuaRawInputInjector({
    resolveSocketPath: () => "\\\\.\\pipe\\mode-cua-raw-input-test",
    callBrokerMethod: async () => {
      calls += 1;
      return { dispatched: 1 };
    },
  });
  await assert.rejects(
    injector(undefined as unknown as unknown[]),
    (error) => error instanceof CuaHelperError && error.code === "invalid_request",
  );
  assert.equal(calls, 0);
});

test("响应门：非 {dispatched} 形状 → helper_unavailable（版本不匹配 fail closed）", async () => {
  const injector = createCuaRawInputInjector({
    resolveSocketPath: () => "\\\\.\\pipe\\mode-cua-raw-input-test",
    callBrokerMethod: async () => ({ dispatched: "two" }),
  });
  await assert.rejects(
    injector([{ k: "move", x: 1, y: 1 }]),
    (error) => error instanceof CuaHelperError && error.code === "helper_unavailable",
  );
});

test("注入调用参数：method/timeout 固定，socketPath 原样透传", async () => {
  const captured: Array<Record<string, unknown>> = [];
  const injector = createCuaRawInputInjector({
    resolveSocketPath: () => "\\\\.\\pipe\\mode-cua-raw-input-test",
    callBrokerMethod: async (args) => {
      captured.push(args as unknown as Record<string, unknown>);
      return { dispatched: 0 };
    },
  });
  const events = [{ k: "wheel", direction: "down", amount: 2, x: 5, y: 6 }];
  assert.deepEqual(await injector(events), { dispatched: 0 });
  assert.deepEqual(captured, [
    {
      socketPath: "\\\\.\\pipe\\mode-cua-raw-input-test",
      method: "raw_input",
      params: { events },
      timeoutMs: 5_000,
    },
  ]);
});
