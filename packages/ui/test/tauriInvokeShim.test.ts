/** Tauri invoke 适配层（spec: docs/specs/tauri-platform-commands.md）。
 * 运行：pnpm exec tsx --test packages/ui/test/tauriInvokeShim.test.ts
 */
import assert from "node:assert/strict";
import { test } from "node:test";

import { DEEP_LINK_EVENT } from "@mode/shared";

import {
  createTauriPlatformService,
  EVENT_SUBSCRIBED_METHODS,
  WINDOW_CONTROL_METHODS,
  M1_IMPLEMENTED_COMMANDS,
  M1_UNIMPLEMENTED_REQUIRED_METHODS,
  normalizeApplicationIconRequest,
  notImplementedUntilM3,
  type TauriEventEnvelope,
  type TauriInvoke,
  type TauriListen,
  type TauriUnlisten,
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

/**
 * 记录订阅的 listen 替身。
 *
 * `holdSetup` 让订阅的 Promise 一直悬着，用来验证「握手必须等订阅装好」这条顺序语义。
 */
function recordingListen(options: { holdSetup?: boolean } = {}) {
  const handlers = new Map<string, Set<(envelope: TauriEventEnvelope<unknown>) => void>>();
  const unlistened: string[] = [];
  const releaseSetup: Array<() => void> = [];

  const listen: TauriListen = <T>(event: string, handler: (e: TauriEventEnvelope<T>) => void) => {
    const set = handlers.get(event) ?? new Set();
    const typed = handler as (envelope: TauriEventEnvelope<unknown>) => void;
    set.add(typed);
    handlers.set(event, set);

    const unlisten: TauriUnlisten = () => {
      set.delete(typed);
      unlistened.push(event);
    };
    if (!options.holdSetup) return Promise.resolve(unlisten);
    return new Promise<TauriUnlisten>((resolve) => {
      releaseSetup.push(() => resolve(unlisten));
    });
  };

  /** 把一条事件投给当前所有订阅者。 */
  const emit = (event: string, payload: unknown): void => {
    for (const handler of handlers.get(event) ?? []) {
      handler({ event, payload });
    }
  };

  const releasePendingSetups = (): void => {
    for (const release of releaseSetup.splice(0)) release();
  };

  return { listen, emit, unlistened, releasePendingSetups };
}

/** 不订阅任何事件的安静替身，给只测命令映射的用例。 */
const noopListen = recordingListen().listen;

test("M1 切片表的 10 条命令都被实现（不是靠抛错桩占位）", () => {
  const { invoke } = recordingInvoke();
  const service = createTauriPlatformService(invoke, noopListen) as Record<string, unknown>;
  assert.equal(M1_IMPLEMENTED_COMMANDS.length, 10);
  for (const name of M1_IMPLEMENTED_COMMANDS) {
    assert.equal(typeof service[name], "function", `${name} 应是函数`);
  }
});

test("命令名逐字映射到 invoke，参数用命令签名的字段名", async () => {
  const { calls, invoke } = recordingInvoke(null);
  const service = createTauriPlatformService(invoke, noopListen);

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
  await new Promise((resolve) => setTimeout(resolve, 0));
  // notifyRendererReady 先等订阅就位再 invoke（顺序语义在下面的专门用例里断言），
  // 因此它在序列里的落点取决于微任务调度，不适合放进「同步命令的先后顺序」断言。
  const synchronousCommands = calls
    .map((call) => call.command)
    .filter((command) => command !== "notifyRendererReady");

  assert.deepEqual(synchronousCommands, [
    "selectDirectory",
    "selectFile",
    "openExternal",
    "syncWindowTabs",
    "syncWindowUnreadCount",
    "openInFileManager",
    "showTaskNotification",
    "saveFile",
  ]);
  assert.ok(
    calls.some((call) => call.command === "notifyRendererReady"),
    "notifyRendererReady 也必须真的发出去",
  );
  // 按命令名取参，不靠下标：序列里混着一次异步落点的握手。
  const argsOf = (command: string) => calls.find((call) => call.command === command)?.args;
  assert.deepEqual(argsOf("openExternal"), { url: "https://example.test/oauth" });
  assert.deepEqual(argsOf("syncWindowTabs"), { paths: ["D:/ws/a", "D:/ws/b"] });
  assert.deepEqual(argsOf("syncWindowUnreadCount"), { count: 3 });
  assert.deepEqual(argsOf("openInFileManager"), { path: "D:/ws" });
  assert.deepEqual(argsOf("showTaskNotification"), {
    payload: { taskId: "t1", status: "completed", title: "标题", body: "正文" },
  });
});

test("getApplicationIcon 把裸字符串归一成结构化 locator 再发", async () => {
  const { calls, invoke } = recordingInvoke(null);
  const service = createTauriPlatformService(invoke, noopListen);
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
  const service = createTauriPlatformService(invoke, noopListen) as Record<string, unknown>;
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
  const service = createTauriPlatformService(invoke, noopListen) as Record<string, unknown>;
  // 这些在 IPlatformService 里带 ? 且**本层没实现**，塞抛错桩会把「不支持就跳过」变成「直接崩」。
  //
  // 注意：这份清单只该列**真的没实现**的可选成员。三个名字先后从它里面挪走了，理由各自如下
  // （不写理由的挪动等于把这条纪律悄悄放宽）：
  // - `onShareImport` / `onOpenWorkspacePath`：宿主确实会发那两个深链 kind，**有真实现**
  //   （见 `EVENT_SUBSCRIBED_METHODS`）；
  // - `getDesktopWindowChromeState`：**有真实现**（M3 Task 4 fix round，见 `WINDOW_CONTROL_METHODS`）
  //   ——它是产品自绘标题栏读初态的成员，用户报的「标题栏点了没反应」里就包括它的缺席。
  for (const name of [
    "selectFiles",
    "printPageToPdf",
    "onBrowserViewRestore",
    "onCloseActiveContextRequest",
    "getUpdateState",
  ]) {
    assert.equal(service[name], undefined, `${name} 应缺席而不是抛错桩`);
  }
});

test("返回 void 的命令：invoke 失败不让进程炸，也不产生未处理的 rejection", async () => {
  const { invoke } = recordingInvoke(null, true);
  const service = createTauriPlatformService(invoke, noopListen);

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

// ------------------------------------------------------------------ 事件通道

test("订阅的事件名取自跨端契约（crates/protocol → 生成物），不是手抄字符串", () => {
  const { listen, unlistened } = recordingListen();
  const subscribed: string[] = [];
  const spyingListen: TauriListen = (event, handler) => {
    subscribed.push(event);
    return listen(event, handler);
  };
  const service = createTauriPlatformService(recordingInvoke().invoke, spyingListen);
  service.onOAuthCallback?.(() => {});
  service.onPaymentCallback?.(() => {});
  assert.deepEqual(subscribed, [DEEP_LINK_EVENT, DEEP_LINK_EVENT]);
  assert.equal(DEEP_LINK_EVENT, "mode:deep-link");
  assert.deepEqual(unlistened, []);
});

test("深链事件按 kind 分流，且每种 kind 只交给它自己的回调（载荷形状各异）", () => {
  const { listen, emit } = recordingListen();
  const service = createTauriPlatformService(recordingInvoke().invoke, listen);
  const oauthUrls: string[] = [];
  const paymentUrls: string[] = [];
  const shareCodes: Array<{ shareCode: string }> = [];
  const workspacePaths: string[] = [];
  service.onOAuthCallback?.((url) => oauthUrls.push(url));
  service.onPaymentCallback?.((url) => paymentUrls.push(url));
  service.onShareImport?.((payload) => shareCodes.push(payload));
  service.onOpenWorkspacePath?.((path) => workspacePaths.push(path));

  emit(DEEP_LINK_EVENT, { kind: "oauth-callback", url: "mode://oauth/callback?state=1" });
  emit(DEEP_LINK_EVENT, { kind: "payment-callback", url: "mode://payment/callback?state=2" });
  emit(DEEP_LINK_EVENT, {
    kind: "share-import",
    url: "mode://share/import?code=abc",
    code: "abc",
  });
  emit(DEEP_LINK_EVENT, {
    kind: "workspace-open",
    url: "mode://workspace/open?path=/a",
    path: "/a",
  });
  // 宿主会 emit 但没有订阅点的 kind 必须被过滤掉（`unrecognized` 是观察用的种类）。
  emit(DEEP_LINK_EVENT, { kind: "unrecognized", url: "mode://nope" });

  assert.deepEqual(oauthUrls, ["mode://oauth/callback?state=1"]);
  assert.deepEqual(paymentUrls, ["mode://payment/callback?state=2"]);
  // `share-import` 交给回调的载荷形状来自声明（`{ shareCode }`），不是把 `code` 直接透传。
  assert.deepEqual(shareCodes, [{ shareCode: "abc" }]);
  assert.deepEqual(workspacePaths, ["/a"]);
});

test("`share-import` / `workspace-open` 缺字段时跳过，不喂半个载荷给回调", () => {
  const { listen, emit } = recordingListen();
  const service = createTauriPlatformService(recordingInvoke().invoke, listen);
  const shareCodes: Array<{ shareCode: string }> = [];
  const workspacePaths: string[] = [];
  service.onShareImport?.((payload) => shareCodes.push(payload));
  service.onOpenWorkspacePath?.((path) => workspacePaths.push(path));

  emit(DEEP_LINK_EVENT, { kind: "share-import", url: "mode://share/import" });
  emit(DEEP_LINK_EVENT, { kind: "share-import", url: "mode://share/import?code=", code: "" });
  emit(DEEP_LINK_EVENT, { kind: "workspace-open", url: "mode://workspace/open" });
  emit(DEEP_LINK_EVENT, { kind: "workspace-open", url: "mode://x", path: 42 });
  emit(DEEP_LINK_EVENT, { kind: "workspace-open", url: "mode://x", path: "" });

  assert.deepEqual(shareCodes, [], "缺 code 的 share-import 不得触发回调（回调会读到 undefined）");
  assert.deepEqual(workspacePaths, [], "缺 path 的 workspace-open 不得触发回调");
});

test("畸形的深链载荷被跳过而不是抛，不影响后续事件", () => {
  const { listen, emit } = recordingListen();
  const service = createTauriPlatformService(recordingInvoke().invoke, listen);
  const urls: string[] = [];
  service.onOAuthCallback?.((url) => urls.push(url));

  emit(DEEP_LINK_EVENT, undefined);
  emit(DEEP_LINK_EVENT, "not-an-object");
  emit(DEEP_LINK_EVENT, { kind: "oauth-callback" });
  emit(DEEP_LINK_EVENT, { kind: "oauth-callback", url: "" });
  emit(DEEP_LINK_EVENT, { kind: "oauth-callback", url: "mode://oauth/callback?state=ok" });

  assert.deepEqual(urls, ["mode://oauth/callback?state=ok"]);
});

test("退订后不再收到事件；订阅未装好就退订也不会留悬挂监听", async () => {
  const { listen, emit, unlistened, releasePendingSetups } = recordingListen({ holdSetup: true });
  const service = createTauriPlatformService(recordingInvoke().invoke, listen);
  const urls: string[] = [];
  const dispose = service.onOAuthCallback?.((url) => urls.push(url));
  assert.ok(dispose);

  // 订阅的 Promise 还悬着就退订：兑现时应当自己退订，而不是把监听留在表里。
  dispose();
  releasePendingSetups();
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.deepEqual(unlistened, [DEEP_LINK_EVENT]);

  emit(DEEP_LINK_EVENT, { kind: "oauth-callback", url: "mode://oauth/callback?state=late" });
  assert.deepEqual(urls, [], "已退订的回调不应再被触发");
});

test("退订是同步返回的（接口契约），且正常路径下真的摘掉监听", async () => {
  const { listen, emit, unlistened } = recordingListen();
  const service = createTauriPlatformService(recordingInvoke().invoke, listen);
  const urls: string[] = [];
  const dispose = service.onOAuthCallback?.((url) => urls.push(url));
  await new Promise((resolve) => setTimeout(resolve, 0));

  emit(DEEP_LINK_EVENT, { kind: "oauth-callback", url: "mode://oauth/callback?state=first" });
  dispose?.();
  emit(DEEP_LINK_EVENT, { kind: "oauth-callback", url: "mode://oauth/callback?state=second" });

  assert.deepEqual(urls, ["mode://oauth/callback?state=first"]);
  assert.deepEqual(unlistened, [DEEP_LINK_EVENT]);
});

test("notifyRendererReady 必须等订阅装好再握手（否则冷启动深链会丢）", async () => {
  const { listen, releasePendingSetups } = recordingListen({ holdSetup: true });
  const { calls, invoke } = recordingInvoke();
  const service = createTauriPlatformService(invoke, listen);

  service.onOAuthCallback?.(() => {});
  service.notifyRendererReady?.();
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.deepEqual(
    calls.map((call) => call.command),
    [],
    "订阅未就位时不得发握手：宿主会立刻补发冷启动深链，此刻没人接",
  );

  releasePendingSetups();
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.deepEqual(calls.map((call) => call.command), ["notifyRendererReady"]);
});

test("窗口控制面（自绘标题栏）真的走到壳：命令带 id、状态读回来、订阅可退订", async () => {
  // 为什么不是「有这个方法就算过」：用户报的就是「按钮点了没反应」——一个**返回 resolve 的
  // 空操作桩**同样能通过纯形状断言。所以这里断言的是**行为**：invoke 真的被调用、
  // 参数是壳的参数名、状态取自壳的返回、订阅返回可调用的同步 disposer。
  const { calls, invoke } = recordingInvoke({
    isMaximized: true,
    supportsNativeRoundedCorners: false,
  });
  const service = createTauriPlatformService(invoke, noopListen);
  for (const name of WINDOW_CONTROL_METHODS) {
    assert.equal(typeof (service as Record<string, unknown>)[name], "function", `${name} 应存在`);
  }

  await service.executeDesktopCommand?.("minimizeWindow");
  assert.deepEqual(
    calls,
    [{ command: "executeDesktopCommand", args: { command: "minimizeWindow" } }],
    "命令 id 必须原样打到壳（空操作桩不会产生这次调用）",
  );

  const state = await service.getDesktopWindowChromeState?.();
  assert.deepEqual(
    state,
    { isMaximized: true, supportsNativeRoundedCorners: false },
    "chrome 状态必须来自壳的返回（自己造一个 false 会让图标永远停在「最大化」）",
  );
  assert.equal(calls[calls.length - 1]?.command, "getDesktopWindowChromeState");

  // 订阅：窗口 API 缺席时按本层纪律**留痕降级**成 no-op disposer（不抛、也不假成功），
  // 且返回的 disposer 可调用（调用方就是这么用的）。
  const dispose = service.onDesktopWindowChromeStateChanged?.(() => {});
  assert.equal(typeof dispose, "function", "订阅必须返回同步 disposer");
  dispose?.();
});

test("事件订阅方法都在服务上真实存在（不是抛错桩）", () => {
  const service = createTauriPlatformService(
    recordingInvoke().invoke,
    noopListen,
  ) as Record<string, unknown>;
  for (const name of EVENT_SUBSCRIBED_METHODS) {
    assert.equal(typeof service[name], "function", `${name} 应是函数`);
    // 事件订阅方法不该出现在「未实现必选」清单里——两处都登记会让抛错桩覆盖真实现。
    assert.ok(
      !(M1_UNIMPLEMENTED_REQUIRED_METHODS as readonly string[]).includes(name),
      `${name} 不应同时登记为未实现`,
    );
    // 也不能出现在命令切片表里：它们走事件，Rust 侧没有同名命令。
    assert.ok(
      !(M1_IMPLEMENTED_COMMANDS as readonly string[]).includes(name),
      `${name} 不是命令`,
    );
  }
});
