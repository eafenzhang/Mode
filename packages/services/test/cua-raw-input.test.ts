import assert from "node:assert/strict";
import net from "node:net";
import test from "node:test";
import { BrokerError, CuaHelperError, mintBrokerSocketPath } from "@mode/cua/broker";

// CUA raw_input 注入端口：helper 准入谓词（唯一理由 = CUA 插件启用）与注入 socket 解析。
// 准入谓词走 ../src/node.js（重装配模块，沿用 cua-helper-admission.test.ts 的动态导入形态）；
// 注入端口直接导入目标模块，socket 与 broker 客户端按依赖注入替换（目录既有 mock 风格）。
const { shouldCreateDefaultCuaProductHelper } = await import("../src/node.js");
const { isCuaExcludedRemoteWorkspaceIdentity } = await import("../src/cuaEnablement.js");
const { createCuaRawInputInjector, createCuaRawInputSocketResolver } =
  await import("../src/cua-permission-broker/cuaRawInput.js");

// ── 准入矩阵（唯一理由 = CUA 插件启用）──
// desktop-local + 无 remote identity + 未注入 resolver 的基准。
const ADMISSION_BASE = {
  serviceAuthorityMode: "desktop-local" as const,
  hasRemoteWorkspaceIdentity: false,
  hasInjectedResolver: false,
  hasBuiltInCuaPlugin: false,
};

test("准入：插件关 → 不建（缺省）", () => {
  assert.equal(shouldCreateDefaultCuaProductHelper({ ...ADMISSION_BASE }), false);
});

test("准入：插件已放行 + desktop-local → 建", () => {
  assert.equal(
    shouldCreateDefaultCuaProductHelper({ ...ADMISSION_BASE, hasBuiltInCuaPlugin: true }),
    true,
  );
});

test("准入：插件开但 authority 不满足（desktop-attached-remote / standalone-server）→ 不建", () => {
  assert.equal(
    shouldCreateDefaultCuaProductHelper({
      ...ADMISSION_BASE,
      serviceAuthorityMode: "desktop-attached-remote",
      hasBuiltInCuaPlugin: true,
    }),
    false,
  );
  assert.equal(
    shouldCreateDefaultCuaProductHelper({
      ...ADMISSION_BASE,
      serviceAuthorityMode: "standalone-server",
      hasBuiltInCuaPlugin: true,
    }),
    false,
  );
});

test("准入：插件开 + ssh/wsl/docker identity → 不建（identity 排除不因插件放宽）", () => {
  for (const identity of [
    "remote:ssh:host:22:alice:/tmp/ws",
    "remote:wsl:Ubuntu:/tmp/ws",
    "remote:docker:abc:/tmp/ws",
  ]) {
    assert.equal(
      shouldCreateDefaultCuaProductHelper({
        ...ADMISSION_BASE,
        hasRemoteWorkspaceIdentity: isCuaExcludedRemoteWorkspaceIdentity(identity),
        hasBuiltInCuaPlugin: true,
      }),
      false,
      identity,
    );
  }
});

test("准入：desktop-local + lan identity 既有行为不回归（插件放行即建）", () => {
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
});

test("准入：插件开但已注入 resolver → 不建（不重复创建）", () => {
  assert.equal(
    shouldCreateDefaultCuaProductHelper({
      ...ADMISSION_BASE,
      hasInjectedResolver: true,
      hasBuiltInCuaPlugin: true,
    }),
    false,
  );
});

// ── 注入 socket 解析：live → 稳定 socket → 注入创建入口（T3c 遗留 #1）──
// 创建入口按依赖注入替换（宿主装配层实际接的是 getOrCreateDefaultCuaProductHelper +
// host.start，同一 admission 与生命周期）；此处只断言触发条件与失败收敛。

const noLive = () => null;
const noStable = async () => null;

test("就绪保障：无 live/stable helper → 触发创建入口，并返回拉起的 socket", async () => {
  const createdSocketPath = mintBrokerSocketPath();
  let ensureCalls = 0;
  const resolver = createCuaRawInputSocketResolver({
    resolveLiveSocketPath: noLive,
    probeStableSocketPath: noStable,
    ensureRemoteDesktopHelper: async () => {
      ensureCalls += 1;
      return createdSocketPath;
    },
  });
  assert.equal(await resolver(), createdSocketPath);
  assert.equal(ensureCalls, 1);
});

test("就绪保障：宿主未注入创建入口 → 不触发、返回 null（probe-only 语义不回归）", async () => {
  let probeCalls = 0;
  const resolver = createCuaRawInputSocketResolver({
    resolveLiveSocketPath: noLive,
    probeStableSocketPath: async () => {
      probeCalls += 1;
      return null;
    },
  });
  assert.equal(await resolver(), null);
  // 既有发现口径不变：无创建入口时仍做 probe（能力位/会话侧不受影响）。
  assert.equal(probeCalls, 1);
});

test("就绪保障：live socket / 稳定 socket 命中时不触发创建入口（既有发现优先复用）", async () => {
  const cases: Array<{
    live: string | null;
    stable: string | null;
    expected: string;
  }> = [
    { live: "\\\\.\\pipe\\mode-cua-live", stable: null, expected: "\\\\.\\pipe\\mode-cua-live" },
    {
      live: null,
      stable: "\\\\.\\pipe\\mode-cua-stable",
      expected: "\\\\.\\pipe\\mode-cua-stable",
    },
  ];
  for (const scenario of cases) {
    let ensureCalls = 0;
    const resolver = createCuaRawInputSocketResolver({
      resolveLiveSocketPath: () => scenario.live,
      probeStableSocketPath: async () => scenario.stable,
      ensureRemoteDesktopHelper: async () => {
        ensureCalls += 1;
        return "\\\\.\\pipe\\mode-cua-created";
      },
    });
    assert.equal(await resolver(), scenario.expected);
    assert.equal(ensureCalls, 0);
  }
});

test("就绪保障：创建失败 → 解析返回 null，注入端口报 helper_unavailable", async () => {
  const resolver = createCuaRawInputSocketResolver({
    resolveLiveSocketPath: noLive,
    probeStableSocketPath: noStable,
    ensureRemoteDesktopHelper: async () => null,
  });
  assert.equal(await resolver(), null);
  const injector = createCuaRawInputInjector({ resolveSocketPath: resolver });
  await assert.rejects(
    injector([{ k: "move", x: 0, y: 0 }]),
    (error) => error instanceof CuaHelperError && error.code === "helper_unavailable",
  );
});

test("就绪保障：创建入口抛错 → 同样收敛为 helper_unavailable，不冒裸错误", async () => {
  const resolver = createCuaRawInputSocketResolver({
    resolveLiveSocketPath: noLive,
    probeStableSocketPath: noStable,
    ensureRemoteDesktopHelper: async () => {
      throw new Error("windows cua runtime resolution failed");
    },
  });
  const injector = createCuaRawInputInjector({ resolveSocketPath: resolver });
  await assert.rejects(
    injector([{ k: "move", x: 0, y: 0 }]),
    (error) =>
      error instanceof CuaHelperError &&
      error.code === "helper_unavailable" &&
      !error.message.includes("runtime resolution failed"),
  );
});

test("就绪保障：并发解析共享同一在途尝试 → 创建入口只被调用一次", async () => {
  let ensureCalls = 0;
  let release: () => void = () => {};
  const gate = new Promise<void>((resolve) => {
    release = resolve;
  });
  const resolver = createCuaRawInputSocketResolver({
    resolveLiveSocketPath: noLive,
    probeStableSocketPath: noStable,
    ensureRemoteDesktopHelper: async () => {
      ensureCalls += 1;
      await gate;
      return "\\\\.\\pipe\\mode-cua-created";
    },
  });
  const first = resolver();
  const second = resolver();
  release();
  assert.deepEqual(await Promise.all([first, second]), [
    "\\\\.\\pipe\\mode-cua-created",
    "\\\\.\\pipe\\mode-cua-created",
  ]);
  assert.equal(ensureCalls, 1);
});

test("就绪保障：失败退避窗口内不重复触发，窗口过后允许再试一次（防止按帧重试轰炸）", async () => {
  let ensureCalls = 0;
  let outcome: string | null = null;
  const resolver = createCuaRawInputSocketResolver({
    resolveLiveSocketPath: noLive,
    probeStableSocketPath: noStable,
    ensureRemoteDesktopHelper: async () => {
      ensureCalls += 1;
      return outcome;
    },
    ensureFailureBackoffMs: 50,
  });
  assert.equal(await resolver(), null);
  assert.equal(ensureCalls, 1);
  // 退避窗口内（模拟注入帧率连续解析）：不再触发创建入口。
  assert.equal(await resolver(), null);
  assert.equal(await resolver(), null);
  assert.equal(ensureCalls, 1);
  await new Promise((resolve) => setTimeout(resolve, 80));
  // 窗口过后按需恢复：helper 经同一创建入口变 live 后，注入恢复可用。
  outcome = "\\\\.\\pipe\\mode-cua-recovered";
  assert.equal(await resolver(), "\\\\.\\pipe\\mode-cua-recovered");
  assert.equal(ensureCalls, 2);
});

test("就绪保障：拉起后的 socket 直接供既有 raw_input 通道使用（线协议不变）", async () => {
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
      socket.write(`${JSON.stringify({ ok: true, result: { dispatched: 1 } })}\n`);
    });
  });
  await new Promise((resolve) => server.listen(socketPath, resolve));
  try {
    const resolver = createCuaRawInputSocketResolver({
      resolveLiveSocketPath: noLive,
      probeStableSocketPath: noStable,
      ensureRemoteDesktopHelper: async () => socketPath,
    });
    const injector = createCuaRawInputInjector({ resolveSocketPath: resolver });
    assert.deepEqual(await injector([{ k: "key", vk: 65, down: true }]), { dispatched: 1 });
    assert.deepEqual(requests, [
      { id: "1", method: "raw_input", params: { events: [{ k: "key", vk: 65, down: true }] } },
    ]);
  } finally {
    await new Promise((resolve) => server.close(resolve));
  }
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
