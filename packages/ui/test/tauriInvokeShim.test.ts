/** Tauri invoke 适配层（spec: docs/specs/tauri-platform-commands.md）。
 * 运行：pnpm exec tsx --test packages/ui/test/tauriInvokeShim.test.ts
 */
import assert from "node:assert/strict";
import { test } from "node:test";

import {
  createTauriPlatformService,
  M1_IMPLEMENTED_COMMANDS,
  M1_UNIMPLEMENTED_REQUIRED_METHODS,
  normalizeApplicationIconRequest,
  notImplementedUntilM3,
  type TauriInvoke,
} from "../src/platform/tauri-invoke-shim.js";

// 让 logger 走静默分支，避免用例输出里混进预期内的 warn。
(globalThis as { __MODE_RENDERER_DISABLE_LOGGING__?: boolean }).__MODE_RENDERER_DISABLE_LOGGING__ =
  true;

type Call = { command: string; args?: Record<string, unknown> };

/** 记录调用的 invoke 替身；`result` 决定返回还是拒绝。 */
function recordingInvoke(result: unknown = null, reject = false) {
  const calls: Call[] = [];
  const invoke: TauriInvoke = (command, args) => {
    calls.push({ command, args });
    return reject ? Promise.reject(new Error(`boom:${command}`)) : Promise.resolve(result);
  };
  return { calls, invoke };
}

test("M1 切片表的 10 条命令都被实现（不是靠抛错桩占位）", () => {
  const { invoke } = recordingInvoke();
  const service = createTauriPlatformService(invoke) as Record<string, unknown>;
  assert.equal(M1_IMPLEMENTED_COMMANDS.length, 10);
  for (const name of M1_IMPLEMENTED_COMMANDS) {
    assert.equal(typeof service[name], "function", `${name} 应是函数`);
  }
});

test("命令名逐字映射到 invoke，参数用命令签名的字段名", async () => {
  const { calls, invoke } = recordingInvoke(null);
  const service = createTauriPlatformService(invoke);

  await service.selectDirectory?.();
  await service.selectFile?.();
  service.openExternal?.("https://example.test/oauth");
  service.syncWindowTabs?.(["D:/ws/a", "D:/ws/b"]);
  service.syncWindowUnreadCount?.(3);
  service.notifyRendererReady?.();
  await service.openInFileManager?.("D:/ws");
  service.showTaskNotification?.({
    taskId: "t1",
    status: "completed",
    title: "标题",
    body: "正文",
  });
  await service.saveFile?.({ data: new Uint8Array([1, 2]) as unknown as ArrayBuffer, suggestedName: "a.bin" } as never);

  assert.deepEqual(
    calls.map((call) => call.command),
    [
      "selectDirectory",
      "selectFile",
      "openExternal",
      "syncWindowTabs",
      "syncWindowUnreadCount",
      "notifyRendererReady",
      "openInFileManager",
      "showTaskNotification",
      "saveFile",
    ],
  );
  assert.deepEqual(calls[2]?.args, { url: "https://example.test/oauth" });
  assert.deepEqual(calls[3]?.args, { paths: ["D:/ws/a", "D:/ws/b"] });
  assert.deepEqual(calls[4]?.args, { count: 3 });
  assert.deepEqual(calls[6]?.args, { path: "D:/ws" });
  assert.deepEqual(calls[7]?.args, {
    payload: { taskId: "t1", status: "completed", title: "标题", body: "正文" },
  });
});

test("getApplicationIcon 把裸字符串归一成结构化 locator 再发", async () => {
  const { calls, invoke } = recordingInvoke(null);
  const service = createTauriPlatformService(invoke);
  await service.getApplicationIcon?.("com.apple.Safari");
  assert.deepEqual(calls[0]?.args, {
    request: {
      locators: [
        { kind: "windows-aumid", value: "com.apple.Safari" },
        { kind: "windows-executable-path", value: "com.apple.Safari" },
        { kind: "darwin-bundle-id", value: "com.apple.Safari" },
      ],
    },
  });
});

test("normalizeApplicationIconRequest：已经是结构化入参时原样透传", () => {
  const structured = { locators: [{ kind: "windows-aumid" as const, value: "x" }] };
  assert.equal(normalizeApplicationIconRequest(structured), structured);
});

test("未实现的**必选**方法调用即抛 not_implemented_until_m3（带方法名）", () => {
  const { invoke } = recordingInvoke();
  const service = createTauriPlatformService(invoke) as Record<string, unknown>;
  assert.ok(M1_UNIMPLEMENTED_REQUIRED_METHODS.length > 0);
  for (const name of M1_UNIMPLEMENTED_REQUIRED_METHODS) {
    assert.equal(typeof service[name], "function", `${name} 应有抛错桩`);
    assert.throws(
      () => (service[name] as () => unknown)(),
      (error: unknown) =>
        error instanceof Error &&
        error.message === `not_implemented_until_m3: ${name}`,
      `${name} 应抛带方法名的 not_implemented_until_m3`,
    );
  }
});

test("可选的未实现方法**保持缺席**，让调用方的 ?. 守卫继续生效", () => {
  const { invoke } = recordingInvoke();
  const service = createTauriPlatformService(invoke) as Record<string, unknown>;
  // 这些在 IPlatformService 里带 ?，塞抛错桩会把「不支持就跳过」变成「直接崩」。
  for (const name of ["selectFiles", "printPageToPdf", "onShareImport", "getUpdateState"]) {
    assert.equal(service[name], undefined, `${name} 应缺席而不是抛错桩`);
  }
});

test("返回 void 的命令：invoke 失败不让进程炸，也不产生未处理的 rejection", async () => {
  const { invoke } = recordingInvoke(null, true);
  const service = createTauriPlatformService(invoke);

  const unhandled: unknown[] = [];
  const onUnhandled = (reason: unknown) => unhandled.push(reason);
  process.on("unhandledRejection", onUnhandled);
  try {
    service.openExternal?.("https://example.test/oauth");
    service.notifyRendererReady?.();
    // 两个微任务轮次足够让 catch 链跑完。
    await new Promise((resolve) => setTimeout(resolve, 0));
    await new Promise((resolve) => setTimeout(resolve, 0));
  } finally {
    process.off("unhandledRejection", onUnhandled);
  }
  assert.deepEqual(unhandled, [], "void 命令的失败必须被 catch 掉");
});

test("notImplementedUntilM3 的文案格式稳定（调用点靠它分流）", () => {
  assert.equal(
    notImplementedUntilM3("openInEditor").message,
    "not_implemented_until_m3: openInEditor",
  );
});
