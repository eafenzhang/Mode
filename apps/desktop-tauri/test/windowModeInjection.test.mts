/**
 * M3 Task 3 · Step 2：`window.mode` 注入的**顺序性质**与**切片一致性**（进 CI：
 * `pnpm --filter @mode/desktop-tauri test`）。
 *
 * ## 为什么这条用例必须存在（DR-T3-1 的代价）
 *
 * Tauri 没有 preload / `contextBridge`，所以 renderer 启动时读的 `window.mode` 只能由**页面加载前**
 * 的注入提供。renderer 的入口模块（`main.tsx` 的顶层语句）在**模块求值期**就建桌面平台服务，
 * 而 `window.mode` 缺席时入口模块**当场抛**、React 永远挂不上——所以
 * 「注入早于 renderer 第一次访问」是**正确性的一部分**，不能靠「一般是够快的」。
 *
 * **这条事实的来源是黑盒对照，不是读渲染层实现**：`productUiMount.test.mts` 的真机负向对照
 * （`MODE_PRODUCT_UI_WITHHOLD_WINDOW_MODE=1`，只撤掉 `window.mode` 这一段注入）观察到的就是
 * 「入口模块抛 + `mounted === false`」。原先这里把「非惰性条件表达式」当作依据，那是读上游衍生
 * 文件实现体得到的结论，已按 clean-room 记为**违规**（spec §19.1）；本注释不再复述那个依据。
 *
 * 所以本用例断言三件事：
 *
 * 1. **顺序**：注入脚本**求值结束时**（= document-start，早于任何 `type="module"` 页面脚本）
 *    `window.mode` 已经可用；而 port / 状态流的投递**仍然**推迟到 `DOMContentLoaded`（spike §4.5
 *    的那条不能被这次改动破坏）。用一个「先读后断言」的探针：先读 `typeof window.mode`，
 *    再断言它非 undefined。
 * 2. **探针本身有敏感性**：同一段探针喂给一个「把赋值挪进 DOMContentLoaded 回调」的变体时，
 *    必须读出 undefined——否则这条断言就是恒真的假绿灯（本仓库反复吃亏的那类失败）。
 * 3. **切片一致**：注入的 `window.mode` 与 M1 的适配层（`@mode/ui/tauri-platform` 的导出切片）
 *    以及 `window.mode` 的**类型声明真源**（`packages/client/src/globals.d.ts`）对得上：
 *    M1 的切片是真实现（同名命令真的 `invoke`），其余**必选**成员显式抛
 *    `not_implemented_until_m3`（M1 口径，不静默返回假值），**可选**成员一律缺席
 *    （调用方本来就用 `?.` 守卫，塞桩会把「平台不支持」变成「直接崩」）。
 *
 * 注入脚本的**文本**不是手抄的：用例从 `crates/sidecar-host/src/bridge.rs` 的 `include_str!`
 * 清单与 `.replace(…)` 清单派生（那两处是注入的真源）。它能抓的是「加了模板文件却没进
 * `include_str!` 清单」与「占了位却没有代入」；**抓不到**「模板进了清单但 `bridge_script()`
 * 忘了拼接它」——那条由 Rust 侧的 `window_mode_is_installed_before_the_rest_of_the_bridge`
 * 与真机用例守（这里刻意不假装覆盖它）。
 */
import { readdirSync, readFileSync } from "node:fs";
import { createContext, runInContext } from "node:vm";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import assert from "node:assert/strict";
import test from "node:test";
import ts from "typescript";

import {
  EVENT_SUBSCRIBED_METHODS,
  M1_IMPLEMENTED_COMMANDS,
  WINDOW_CONTROL_METHODS,
  M1_UNIMPLEMENTED_REQUIRED_METHODS,
  normalizeApplicationIconRequest,
} from "@mode/ui/tauri-platform";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "../../..");
const read = (relativePath: string): string =>
  readFileSync(resolve(repoRoot, relativePath), "utf8");

const BRIDGE_RUST = "crates/sidecar-host/src/bridge.rs";
/** 壳侧窗口控制面的实现（M3 Task 4 fix round 的 `rename` 字面量真源之一）。 */
const WINDOW_CONTROLS_RUST = "apps/desktop-tauri/src-tauri/src/window_controls.rs";
const MODE_WINDOW_DECLARATION = "packages/client/src/globals.d.ts";
/** 深链事件名与 kind 的**生成物**真源（`crates/protocol` 单向导出；CI 用 `--check` 防漂移）。 */
const GENERATED_PROTOCOL = "packages/shared/src/generated/tauri-protocol.ts";
const TEST_STARTUP_ID = "host-1234-5";

/** 从生成物里读深链事件名（不 import `@mode/shared` 的 barrel：本包没有那条依赖）。 */
function readDeepLinkEventName(): string {
  const match = new RegExp(/export const DEEP_LINK_EVENT = "([^"]+)"/).exec(read(GENERATED_PROTOCOL));
  assert.ok(match, `${GENERATED_PROTOCOL} 里应导出 DEEP_LINK_EVENT`);
  return match![1]!;
}

/** 从生成物的 kind 联合里读全部 kind（注入脚本订阅的 kind 必须是其中之一）。 */
function readDeepLinkKinds(): string[] {
  const line = read(GENERATED_PROTOCOL)
    .split("\n")
    .find((candidate) => candidate.startsWith("export type DeepLinkEventKind ="));
  assert.ok(line, `${GENERATED_PROTOCOL} 里应声明 DeepLinkEventKind`);
  return [...line!.matchAll(/"([a-z-]+)"/g)].map((match) => match[1]!);
}

/** 注入脚本里被 `window.mode` 明确**留空**的必选成员（每条都要有理由，见下面的用例）。 */
const DELIBERATELY_ABSENT_REQUIRED_MEMBERS = ["log"];

// ---------------------------------------------------------------------------
// 从注入的真源（bridge.rs）派生「壳实际注入的那段脚本」
// ---------------------------------------------------------------------------

interface InjectionManifest {
  templates: string[];
  placeholders: Array<{ placeholder: string; value: string }>;
}

/** 把 `bridge.rs` 里的 `include_str!("x")` 与 `.replace("__X__", expr)` 解出来。 */
function readInjectionManifest(bridgeRs: string): InjectionManifest {
  // 只看生产代码：`#[cfg(test)]` 段里的用例也会写 `.replace(...)`，那不是注入清单
  // （把测试代码当清单读，症状是「用例自己把清单解析搞坏」）。
  const production = bridgeRs.split("#[cfg(test)]")[0]!;
  const templates = [...production.matchAll(/include_str!\("([^"]+)"\)/g)].map((match) => match[1]!);
  assert.ok(templates.length > 0, `${BRIDGE_RUST} 里应有 include_str! 的模板清单`);

  const consts = new Map<string, string>();
  // wire 名既有 snake_case（Host 通道的四个）也有 camelCase（窗口控制面那两个，M1 的 rename
  // 契约要求命令名与 IPlatformService 逐字一致 → 就是 camelCase）。这里按**标识符**的字符集收，
  // 不要写成 `[a-z_]+`：那会让 camelCase 的命令在清单解析里静默消失（本用例自己踩过一次）。
  for (const match of production.matchAll(/pub const (CMD_\w+): &str = "([A-Za-z][A-Za-z0-9_]*)"/g)) {
    consts.set(match[1]!, match[2]!);
  }

  const placeholders = [...production.matchAll(/\.replace\("(__MODE_\w+__)",\s*([^)]+)\)/g)].map(
    (match) => {
      const placeholder = match[1]!;
      const expression = match[2]!.trim();
      // startupId 那个占位符由用例给值；按**占位符名**判断而不是按代入表达式的写法，
      // 这样重命名 Rust 侧的局部变量不会把用例搞红（`sanitized_startup_id` 撞过一次）。
      if (placeholder === "__MODE_HOST_STARTUP_ID__") {
        return { placeholder, value: TEST_STARTUP_ID };
      }
      const referenced = [...consts.keys()].find((name) => expression.includes(name));
      assert.ok(
        referenced,
        `${BRIDGE_RUST} 里 ${placeholder} 的代入表达式 \`${expression}\` 解析不出来（用例需要知道它代入了什么）`,
      );
      return { placeholder, value: consts.get(referenced!)! };
    },
  );
  assert.ok(placeholders.length > 0, `${BRIDGE_RUST} 里应有 .replace(...) 的占位符代入清单`);
  return { templates, placeholders };
}

/** 按真源清单拼出壳实际注入的那段脚本（= `bridge_script()` 的等价物）。 */
function emitInjectedScript(): string {
  const bridgeRs = read(BRIDGE_RUST);
  const manifest = readInjectionManifest(bridgeRs);
  // 模板路径按**仓库相对**拼接：`resolve()` 会以 `process.cwd()` 为基准，
  // 而这条用例在 `pnpm --filter @mode/desktop-tauri test` 下 cwd 是包目录
  // （从仓库根跑会歪打正着地通过——那正是这种 bug 藏得住的原因）。
  const templateDir = BRIDGE_RUST.split("/").slice(0, -1).join("/");
  const scripts = manifest.templates.map((relative) => read(`${templateDir}/${relative}`));
  let emitted = scripts.join("\n");
  for (const { placeholder, value } of manifest.placeholders) {
    emitted = emitted.split(placeholder).join(value);
  }
  // 只扫**真源登记的**占位符：脚本里还有 `window.__MODE_HOST_BRIDGE__` 这种诊断全局，
  // 它长得像占位符但不是（用宽正则会把诊断面误判成残留）。
  for (const { placeholder } of manifest.placeholders) {
    assert.ok(
      !emitted.includes(placeholder),
      `注入脚本里不得残留占位符 ${placeholder}（残留会让页面拿到字面量，invoke 必然 not found）`,
    );
  }
  return emitted;
}

// ---------------------------------------------------------------------------
// 一个够用的「页面」：document-start 求值，DOMContentLoaded 再跑延迟投递
// ---------------------------------------------------------------------------

interface Page {
  evaluate(script: string): void;
  /** 放行所有到期的定时器回调（去抖路径用；顺序按注册顺序，足够确定）。 */
  fireTimers(): void;
  /** 读一个全局表达式（探针用法：先读，后断言）。 */
  probe(expression: string): unknown;
  fireDomContentLoaded(): void;
  flushMicrotasks(): Promise<void>;
  postMessages: Array<{ payload: unknown; transferred: unknown[] }>;
  invokes: Array<{ command: string; args: unknown }>;
  /** 注入脚本注册过的深链监听（I2 的配对断言用）。 */
  deepLinkListeners: Array<{ event: string; handler: (envelope: unknown) => void }>;
  /** 页面里建过的 `MessageChannel`（LR-4 scoped port 断言用）。 */
  createdChannels: Array<{ port1: FakePort; port2: FakePort }>;
  /** 派发一条 `window` message（LR-4 ready ACK 断言用）。 */
  dispatchWindowMessage(data: unknown): void;
  windowMessageListenerCount(): number;
  readyState(): string;
}

/**
 * 端口替身。
 *
 * 用替身而不是 Node 的真 `MessageChannel`：真端口会握住事件循环的句柄，
 * `node --test` 就会等它而**不退出**（表现是「用例全绿但整套挂住」）；
 * 而端口自身的语义（真 `MessagePort`、`event.source === window`）由 opt-in 的真 webview 用例负责，
 * 这里要验的是**注入顺序**与**投递时机**，替身足够且确定性更好。
 */
class FakePort {
  listeners: Array<(event: { data: unknown }) => void> = [];
  posted: unknown[] = [];
  started = false;

  addEventListener(type: string, handler: (event: { data: unknown }) => void): void {
    if (type === "message") this.listeners.push(handler);
  }

  removeEventListener(): void {}

  start(): void {
    this.started = true;
  }

  close(): void {}

  postMessage(data: unknown): void {
    this.posted.push(data);
  }

  deliver(data: unknown): void {
    for (const listener of this.listeners) listener({ data });
  }
}

function createPage(): Page {
  const postMessages: Page["postMessages"] = [];
  const invokes: Page["invokes"] = [];
  const domContentLoaded: Array<() => void> = [];

  const sandbox: Record<string, unknown> = {};
  sandbox.window = sandbox;
  sandbox.globalThis = sandbox;
  sandbox.self = sandbox;
  sandbox.console = console;
  sandbox.Uint8Array = Uint8Array;
  const createdChannels: Array<{ port1: FakePort; port2: FakePort }> = [];
  const windowMessageListeners: Array<(event: { source: unknown; data: unknown }) => void> = [];
  sandbox.MessageChannel = function MessageChannelStub() {
    const created = { port1: new FakePort(), port2: new FakePort() };
    createdChannels.push(created);
    return created;
  };
  sandbox.TextEncoder = TextEncoder;
  sandbox.TextDecoder = TextDecoder;
  sandbox.addEventListener = (type: string, handler: (event: { source: unknown; data: unknown }) => void) => {
    if (type === "message") windowMessageListeners.push(handler);
  };
  sandbox.CustomEvent = CustomEvent;
  // 测试里不让定时器自己跑：时序要能被显式控制，否则断言变成「等一会儿再看」。
  // 但**要能手动放行**（窗口 chrome 状态那条订阅是去抖的：没有这一步就测不到它的触发路径）。
  let nextTimerId = 1;
  const timers = new Map<number, () => void>();
  sandbox.setTimeout = (callback: () => void) => {
    const id = nextTimerId;
    nextTimerId += 1;
    timers.set(id, callback);
    return id;
  };
  sandbox.clearTimeout = (id: number) => {
    timers.delete(id);
  };
  sandbox.document = {
    readyState: "loading",
    addEventListener: (type: string, handler: () => void) => {
      if (type === "DOMContentLoaded") domContentLoaded.push(handler);
    },
  };
  sandbox.postMessage = (payload: unknown, _target: string, transferred?: unknown[]) => {
    postMessages.push({ payload, transferred: transferred ?? [] });
  };
  sandbox.dispatchEvent = () => true;
  const recordInvoke = (command: string, args?: unknown): Promise<unknown> => {
    invokes.push({ command, args });
    if (command === "host_recv") return new Promise(() => {});
    // 窗口 chrome 状态：壳的返回形状是 `{ isMaximized, … }`，替身也得给形状
    // （注入层对形状做校验：给 null 会被判成「读取失败」，那条路径单独有用例）。
    if (command === "getDesktopWindowChromeState") {
      // 像真 IPC 一样回一份**拷贝**（结构化克隆语义）：直接把替身对象交出去会让
      // 「改替身」与「改上一份读到的状态」变成同一件事，去重逻辑的断言就失去意义。
      const source =
        (sandbox.__chrome as Record<string, unknown> | undefined) ??
        (((sandbox.__chrome = {
          isMaximized: false,
          macOSMajorVersion: null,
          supportsNativeRoundedCorners: false,
        }) as unknown) as Record<string, unknown>);
      return Promise.resolve({ ...source });
    }
    return Promise.resolve(null);
  };
  sandbox.__TAURI_INTERNALS__ = { invoke: recordInvoke };
  // 事件订阅要能**被喂**：I2 的 kind↔方法配对断言靠它（只断言「两个 kind 字符串出现过」是抓不到对调的）。
  const listenCalls: Array<{ event: string; handler: (envelope: unknown) => void }> = [];
  sandbox.__TAURI__ = {
    core: { invoke: recordInvoke },
    event: {
      listen: (event: string, handler: (envelope: unknown) => void) => {
        listenCalls.push({ event, handler });
        return Promise.resolve(() => {});
      },
    },
  };

  const context = createContext(sandbox);
  /** VM 里的 `window` 与宿主 sandbox 可能不是同一个对象；派发事件时用 VM 自己的那个。 */
  const vmWindow = runInContext("window", context);
  return {
    evaluate: (script: string) => {
      runInContext(script, context);
    },
    probe: (expression: string) => runInContext(expression, context),
    fireTimers: () => {
      const pending = [...timers.values()];
      timers.clear();
      for (const callback of pending) callback();
    },
    fireDomContentLoaded: () => {
      (sandbox.document as { readyState: string }).readyState = "complete";
      for (const handler of domContentLoaded.splice(0)) handler();
    },
    flushMicrotasks: () => new Promise((resolveFlush) => setImmediate(resolveFlush)),
    postMessages,
    invokes,
    deepLinkListeners: listenCalls,
    createdChannels,
    dispatchWindowMessage: (data: unknown) => {
      for (const listener of windowMessageListeners) {
        listener({ source: vmWindow, data });
      }
    },
    windowMessageListenerCount: () => windowMessageListeners.length,
    readyState: () => (sandbox.document as { readyState: string }).readyState,
  };
}

/** 「先读后断言」的探针：在**脚本求值结束时**读 `window.mode`，返回值交给断言。 */
function readWindowModeAtDocumentStart(script: string): unknown {
  const page = createPage();
  page.evaluate(script);
  const observed = page.probe("typeof window.mode");
  assert.equal(page.readyState(), "loading", "探针必须发生在 DOMContentLoaded 之前");
  return observed;
}

// ---------------------------------------------------------------------------
// `window.mode` 的类型声明真源
// ---------------------------------------------------------------------------

/**
 * `window.mode` 的声明形态。
 *
 * `requiredWithoutConsumedValue` 是**机械**算出来的：声明返回 `void` / `Promise<void>` /
 * 函数类型（disposer）的成员，调用方拿不到要读的值，所以注入层对它们只该降级为空操作；
 * 其余（`Promise<T>`，T ≠ void）会返回被消费的值，必须显式抛（返回假值会把「能力不存在」
 * 伪装成事实）。
 */
interface ModeDeclaration {
  required: string[];
  optional: string[];
  requiredThrowing: string[];
  requiredWithoutConsumedValue: string[];
}

/** 声明的返回形态是否「没有要消费的值」。 */
function returnsNoConsumedValue(member: ts.MethodSignature | ts.PropertySignature): boolean {
  const declared = member.type;
  if (!declared) return false;
  if (ts.isFunctionTypeNode(declared)) return true; // `() => void`：disposer
  if (declared.kind === ts.SyntaxKind.VoidKeyword) return true; // `void`
  if (ts.isTypeReferenceNode(declared)) {
    const name = declared.typeName.getText();
    const first = declared.typeArguments?.[0];
    // `Promise<void>` / `PromiseLike<void>`
    if ((name === "Promise" || name === "PromiseLike") && first?.kind === ts.SyntaxKind.VoidKeyword) {
      return true;
    }
  }
  return false;
}

function readWindowModeDeclaration(): ModeDeclaration {
  const source = read(MODE_WINDOW_DECLARATION);
  const file = ts.createSourceFile(
    MODE_WINDOW_DECLARATION,
    source,
    ts.ScriptTarget.Latest,
    true,
    ts.ScriptKind.TS,
  );
  let modeType: ts.TypeLiteralNode | null = null;
  const visit = (node: ts.Node): void => {
    if (ts.isInterfaceDeclaration(node) && node.name.text === "Window") {
      for (const member of node.members) {
        if (
          ts.isPropertySignature(member) &&
          member.name?.getText(file) === "mode" &&
          member.type &&
          ts.isTypeLiteralNode(member.type)
        ) {
          modeType = member.type;
        }
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(file);
  assert.ok(modeType, `${MODE_WINDOW_DECLARATION} 里应能找到 interface Window 的 mode 成员`);
  const required: string[] = [];
  const optional: string[] = [];
  const requiredThrowing: string[] = [];
  const requiredWithoutConsumedValue: string[] = [];
  for (const member of (modeType as ts.TypeLiteralNode).members) {
    // `window.mode` 的成员大多是 `MethodSignature`（`foo(): Promise<…>`），只有少数是属性签名。
    if ((!ts.isMethodSignature(member) && !ts.isPropertySignature(member)) || !member.name) continue;
    const name = member.name.getText(file);
    if (member.questionToken) {
      optional.push(name);
      continue;
    }
    required.push(name);
    (returnsNoConsumedValue(member) ? requiredWithoutConsumedValue : requiredThrowing).push(name);
  }
  assert.ok(required.length > 0 && optional.length > 0, "window.mode 应有必选与可选两类成员");
  assert.ok(
    requiredThrowing.length > 0 && requiredWithoutConsumedValue.length > 0,
    "必选成员应同时存在两类返回形态（否则分类断言是空的）",
  );
  return { required, optional, requiredThrowing, requiredWithoutConsumedValue };
}

/**
 * 第二处声明真源：`IPlatformService`（`packages/shared/src/platform.ts`）。
 *
 * 为什么需要（review I1）：渲染层引用的 123 个 `window.mode` 成员里有 11 个**不在**
 * `globals.d.ts` 里——它们由 `IPlatformService` 声明、被适配层直接透传。只读一处声明，
 * 「消费者在另一个文件里调用」的那批就没人守。返回 `name → 是否「没有要读的值」`，
 * 分类复用同一个形态判据。
 */
function readPlatformServiceDeclaration(): Map<string, boolean> {
  const relative = "packages/shared/src/platform.ts";
  const file = ts.createSourceFile(
    relative,
    read(relative),
    ts.ScriptTarget.Latest,
    true,
    ts.ScriptKind.TS,
  );
  const found = new Map<string, boolean>();
  const visit = (node: ts.Node): void => {
    if (ts.isInterfaceDeclaration(node) && node.name.text === "IPlatformService") {
      for (const member of node.members) {
        if ((!ts.isMethodSignature(member) && !ts.isPropertySignature(member)) || !member.name) {
          continue;
        }
        found.set(member.name.getText(file), returnsNoConsumedValue(member));
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(file);
  assert.ok(found.size > 30, `IPlatformService 的成员只读到 ${found.size} 个，解析可能没命中`);
  return found;
}

// ---------------------------------------------------------------------------
// 用例
// ---------------------------------------------------------------------------

test("注入的 window.mode 在页面脚本之前就位：document-start 探针能读到它", () => {
  const observed = readWindowModeAtDocumentStart(emitInjectedScript());
  assert.equal(
    observed,
    "object",
    "注入脚本求值结束时 window.mode 必须已经可用（renderer 入口模块求值期就会读它，" +
      "晚一步就是入口模块当场抛、React 挂不上——DR-T3-1 的顺序性质）",
  );
});

test("探针有敏感性：把赋值挪进 DOMContentLoaded 的变体必须读不到 window.mode", () => {
  const late = [
    "(function () {",
    '  if (document.readyState === "loading") {',
    '    document.addEventListener("DOMContentLoaded", function () { window.mode = {}; });',
    "  } else { window.mode = {}; }",
    "})();",
  ].join("\n");
  assert.equal(
    readWindowModeAtDocumentStart(late),
    "undefined",
    "同一段探针在「晚注入」变体上必须读出 undefined——否则前一条断言是恒真的假绿灯",
  );
});

test("port 与状态流的投递仍然推迟到 DOMContentLoaded（spike §4.5 不能被这次改动破坏）", async () => {
  const page = createPage();
  page.evaluate(emitInjectedScript());
  assert.equal(
    page.postMessages.length,
    0,
    "渲染层的 message 监听注册在模块脚本里：提前投递会白丢一次，所以 DOMContentLoaded 前不得投",
  );
  page.fireDomContentLoaded();
  await page.flushMicrotasks();
  const channelTypes = page.postMessages.map(
    (message) => (message.payload as { type?: string }).type ?? "<invalid>",
  );
  assert.ok(
    channelTypes.includes("mode:service-port"),
    `DOMContentLoaded 后必须投递服务端口，实际投了：${channelTypes.join(", ")}`,
  );
  const portMessage = page.postMessages.find(
    (message) => (message.payload as { type?: string }).type === "mode:service-port",
  )!;
  assert.equal(
    portMessage.transferred.length,
    1,
    "端口必须以 transferable 形态交给渲染层（`window.postMessage(payload, \"*\", [port1])`）",
  );
  const rendererPort = portMessage.transferred[0] as FakePort;
  assert.ok(rendererPort instanceof FakePort, "转移的必须是页面内建的端口对象");
  assert.equal(
    rendererPort.started,
    false,
    "交给渲染层的那端由渲染层自己 start()，注入脚本不得替它启动",
  );
});

test("LR-4 scoped port：建端口、投递元数据、出方向帧带 scope 前缀", async () => {
  const page = createPage();
  page.evaluate(emitInjectedScript());
  page.fireDomContentLoaded();
  await page.flushMicrotasks();

  const baseChannelCount = page.createdChannels.length;
  assert.equal(
    page.probe(
      `window.__MODE_HOST_BRIDGE__.createScopedServicePort({
         scopeId: 7,
         attachmentId: "attachment-1",
         sessionId: "session-1",
         target: { kind: "lan", serverId: "server-1" },
         capabilities: { computerUse: true }
       })`,
    ),
    true,
  );
  assert.equal(page.createdChannels.length, baseChannelCount + 1, "必须新建一个页面端口");

  const scopedMessage = page.postMessages.find(
    (message) =>
      (message.payload as { type?: string }).type === "mode:scoped-service-port",
  );
  assert.ok(scopedMessage, "scoped 端口必须以既有频道投递");
  assert.equal(scopedMessage.transferred.length, 1);
  const payload = scopedMessage.payload as {
    attachmentId?: string;
    sessionId?: string;
    target?: { kind?: string };
    capabilities?: { computerUse?: boolean };
  };
  assert.equal(payload.attachmentId, "attachment-1");
  assert.equal(payload.sessionId, "session-1");
  assert.equal(payload.target?.kind, "lan");
  assert.equal(payload.capabilities?.computerUse, true);

  // 出方向：renderer 侧端口发一帧，桥必须加 `0xFF + scopeId(u32 BE)` 前缀再送壳。
  const scopedChannel = page.createdChannels.at(-1);
  assert.ok(scopedChannel);
  scopedChannel.port2.deliver(Uint8Array.of(100, 1, 2, 3));
  await page.flushMicrotasks();
  const sent = [...page.invokes].reverse().find((entry) => entry.command === "host_send");
  assert.ok(sent, "scoped 帧必须经 host_send 出去");
  const frame = sent.args as Uint8Array;
  assert.ok(frame instanceof Uint8Array);
  assert.equal(frame[0], 0xff);
  assert.equal(
    new DataView(frame.buffer, frame.byteOffset, frame.byteLength).getUint32(1, false),
    7,
  );
  assert.deepEqual([...frame.subarray(5)], [100, 1, 2, 3]);

  // renderer ready ACK → 桥发 scope-ready 控制帧（Host 收到后才发 Initialize）。
  assert.ok(page.windowMessageListenerCount() >= 1, "桥必须注册 window message 监听");
  page.dispatchWindowMessage({
    type: "mode:scoped-service-port-ready",
    attachmentId: "attachment-1",
    sessionId: "session-1",
  });
  await page.flushMicrotasks();
  const controlFrames = page.invokes
    .filter((entry) => entry.command === "host_send")
    .map((entry) => entry.args)
    .filter(
      (args): args is Uint8Array => args instanceof Uint8Array && args[0] === 0xfe,
    );
  assert.equal(controlFrames.length, 1, "ready ACK 必须翻译成一条控制帧");
  assert.deepEqual(
    JSON.parse(new TextDecoder().decode(controlFrames[0]!.subarray(1))),
    { type: "scope-ready", scopeId: 7 },
  );

  assert.equal(page.probe(`window.__MODE_HOST_BRIDGE__.releaseScopedServicePort(7)`), true);
  assert.equal(page.probe(`window.__MODE_HOST_BRIDGE__.releaseScopedServicePort(7)`), false);
});

/**
 * 渲染层的 `window.mode` 使用面：从**消费者**一侧枚举，而不是从声明文件一侧。
 *
 * 为什么需要它（review I1）：`packages/client/src/globals.d.ts` 是 `window.mode` 的声明真源，
 * 但渲染层实际引用了 **123** 个成员、其中 **11** 个没有在那份声明里（它们由
 * `IPlatformService` 声明、由适配层直接透传）。只守声明面就会漏掉「消费者在另一个文件里
 * 直接调用」的那些——那正是本仓库反复吃亏的那类缺口。
 *
 * 分类按**调用形态**（不是按名字）：跟一个成员名后面的是 `(`（或 `!(`）时，缺席就会抛
 * `TypeError: ... is not a function`，所以它**必须被覆盖**；`?.`、`!`前的属性读、以及
 * `window.mode.X ? … : undefined` 这类存在性判断在缺席时都是安全的（属性读到 `undefined`
 * 不会抛）。
 */
function readRendererWindowModeUsage(): {
  unsafeCalls: string[];
  /** 用了 `!`（非空断言）调用的成员，但**同一个文件里没有存在性判断**——那才是真的没守卫。 */
  unguardedNonNullCalls: string[];
  safeOnly: string[];
  fileCount: number;
} {
  // 两个根都扫：运行时它们是同一个渲染层 bundle（适配层在 renderer，`logger` 等在 ui）。
  const roots = [
    resolve(repoRoot, "packages/desktop/src/renderer"),
    resolve(repoRoot, "packages/ui/src"),
  ];
  const files: string[] = [];
  const walk = (directory: string): void => {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const full = join(directory, entry.name);
      if (entry.isDirectory()) walk(full);
      else if (/\.(ts|tsx)$/.test(entry.name)) files.push(full);
    }
  };
  for (const root of roots) walk(root);

  const unsafe = new Set<string>();
  const safe = new Set<string>();
  const nonNull = new Set<string>();
  const unguardedNonNull = new Set<string>();
  for (const file of files) {
    const source = readFileSync(file, "utf8");
    /** 同一文件里出现过 `window.mode.X ? …` / `?? ` / `&&` / `typeof window.mode.X` —— 缺席已被处理。 */
    const presenceTested = new Set<string>();
    for (const match of source.matchAll(
      /window\.mode\.([A-Za-z_$][\w$]*)\s*(?:\?\?|&&|\?(?![.?]))|typeof window\.mode\.([A-Za-z_$][\w$]*)/g,
    )) {
      presenceTested.add((match[1] ?? match[2])!);
    }
    for (const match of source.matchAll(/window\.mode\.([A-Za-z_$][\w$]*)/g)) {
      const name = match[1]!;
      const tail = source.slice(match.index + match[0].length, match.index + match[0].length + 4);
      if (/^\s*!\s*\(/.test(tail)) {
        nonNull.add(name);
        // `X ? () => X!() : undefined` 是**有守卫**的：三元为假时那个箭头根本不会被创建。
        if (!presenceTested.has(name)) unguardedNonNull.add(name);
      } else if (/^\s*\(/.test(tail)) {
        unsafe.add(name);
      } else {
        safe.add(name);
      }
    }
  }
  return {
    unsafeCalls: [...unsafe].sort(),
    unguardedNonNullCalls: [...unguardedNonNull].sort(),
    safeOnly: [...safe].filter((name) => !unsafe.has(name) && !nonNull.has(name)).sort(),
    fileCount: files.length,
  };
}

test("平台面清单会在 DOMContentLoaded 后上报给壳（入参形状对得上，I3 的可见面）", async () => {
  // review I3 的机制断言：清单必须**离开页面**（release 里没有 devtools/console），
  // 而这条 invoke 的**入参形状**是它成不成立的关键——Tauri v2 按 `#[tauri::command]` 的
  // 参数名映射，壳侧是 `host_platform_report(report: …)`，所以必须是 `{ report: {…} }`。
  // 真机侧的同一条断言在 `productUiMount.test.mts`（看壳的 stderr 里有没有那行）。
  const page = createPage();
  page.evaluate(emitInjectedScript());
  page.fireDomContentLoaded();
  await page.flushMicrotasks();

  const call = page.invokes.find((candidate) => candidate.command === "host_platform_report");
  assert.ok(call, `DOMContentLoaded 后必须上报平台面清单，实际调用：${JSON.stringify(page.invokes)}`);
  const args = JSON.parse(JSON.stringify(call.args)) as {
    report?: Record<string, unknown>;
  };
  assert.ok(args.report, `入参必须包在 \`report\` 参数名里（Tauri 按参数名映射）：${JSON.stringify(args)}`);
  const report = args.report!;
  for (const field of ["implemented", "notImplemented", "noOp", "deliberatelyAbsent", "degraded"]) {
    assert.ok(Array.isArray(report[field]), `清单里必须有 ${field}（与壳侧的 PlatformSurfaceReport 对应）`);
  }
  assert.ok(
    (report.implemented as string[]).includes("selectDirectory"),
    "清单里的 implemented 必须是注入层的真实切片",
  );
  assert.ok(
    (report.notImplemented as string[]).includes("connectRemote"),
    "清单里的 notImplemented（要读值的那类）必须与注入层一致",
  );

  // 降级**事后**发生（渲染层挂载时才调），所以更新一条记录是可观察的：再调一次被降级的方法，
  // 应当触发第二次上报（带上新的 degraded 条目）。
  page.probe("window.mode.onFocusTab(function () {});");
  await page.flushMicrotasks();
  const reports = page.invokes.filter((candidate) => candidate.command === "host_platform_report");
  assert.ok(
    reports.length >= 2,
    "出现新的降级理由时必须再报一条（否则清单永远停在 DOMContentLoaded 那一刻的空账本）",
  );
  const latest = JSON.parse(JSON.stringify(reports[reports.length - 1]!.args)) as {
    report: { degraded: string[] };
  };
  assert.ok(
    latest.report.degraded.includes("not_implemented_noop:onFocusTab"),
    `最后一次上报必须带上新的降级条目：${JSON.stringify(latest.report.degraded)}`,
  );
});

test("渲染层**直接调用**（没有 ?. 守卫）的 window.mode 成员必须全部被注入覆盖", () => {
  const page = createPage();
  page.evaluate(emitInjectedScript());
  const injected = new Set(Array.from(page.probe("Object.keys(window.mode)") as string[]));
  // 故意留空的成员也在 `window.mode` 上不存在，所以单独把它算进口径（它是显式决策，见下一组用例）。
  for (const name of DELIBERATELY_ABSENT_REQUIRED_MEMBERS) injected.delete(name);

  const usage = readRendererWindowModeUsage();
  // 先证扫描本身不是空转：目录扫不到文件、或者只扫到几个名字时，下面的断言会变成假绿灯。
  assert.ok(
    usage.fileCount > 1000,
    `渲染层源码文件数异常（扫到 ${usage.fileCount} 个），扫描根可能错了`,
  );
  assert.ok(
    usage.unsafeCalls.length >= 30,
    `直接调用的成员只扫到 ${usage.unsafeCalls.length} 个，扫描规则可能坏了：${usage.unsafeCalls.join(", ")}`,
  );
  assert.ok(
    usage.unsafeCalls.length + usage.safeOnly.length >= 100,
    `引用到的成员总数 ${usage.unsafeCalls.length + usage.safeOnly.length} 明显偏少（正常 ≥120）`,
  );

  // `!` 断言式调用（`X ? () => X!() : undefined`）只有在**同一文件里没有存在性判断**时才算没守卫；
  // 这条把上面的启发式变成可检查的不变式（否则「带 ! 的调用一律放过」会随时间变成漏网）。
  assert.deepEqual(
    usage.unguardedNonNullCalls,
    [],
    `这些成员用非空断言调用了，但同一文件里没有存在性判断——那等于没有守卫，` +
      `要么补守卫，要么把它们纳入覆盖：${usage.unguardedNonNullCalls.join(", ")}`,
  );

  const uncovered = usage.unsafeCalls.filter((name) => !injected.has(name));
  assert.deepEqual(
    uncovered,
    [],
    `渲染层会不加守卫地调这些成员，但注入里没有它们——在 Tauri 壳里会抛 ` +
      `"TypeError: window.mode.X is not a function" 且不留痕（review I1）。` +
      `按返回形态补进 window_mode.ts 的 REJECTING_REQUIRED（要读值）或 NO_OP_REQUIRED（没有要读的值）：` +
      `${uncovered.join(", ")}`,
  );
});

test("注入的 window.mode 与 M1 适配层切片一致：命令同名同参，事件订阅走深链事件", async () => {
  const page = createPage();
  page.evaluate(emitInjectedScript());
  const names = page.probe("Object.keys(window.mode).sort()") as string[];

  const implemented: string[] = [...M1_IMPLEMENTED_COMMANDS];
  for (const command of implemented) {
    assert.ok(
      names.includes(command),
      `M1 切片里的 ${command} 必须出现在注入的 window.mode 上（切片表是 M1 的真源）`,
    );
  }
  // 真调用必须真的走壳命令，而不是返回假值。
  await page.probe("window.mode.selectDirectory()");
  await page.flushMicrotasks();
  assert.deepEqual(
    page.invokes.filter((call) => call.command === "selectDirectory").length,
    1,
    "window.mode.selectDirectory() 必须真的 invoke 同名壳命令",
  );

  // 事件订阅：订阅的必须是宿主的深链事件，且两条方法的 kind 与生成物一致。
  const modeWindow = read("crates/sidecar-host/src/window_mode.ts");
  const deepLinkEvent = readDeepLinkEventName();
  assert.ok(
    modeWindow.includes(`"${deepLinkEvent}"`),
    `注入脚本必须订阅宿主 emit 的深链事件 "${deepLinkEvent}"（写错即静默收不到）`,
  );
  const kinds = readDeepLinkKinds();
  for (const kind of ["oauth-callback", "payment-callback"]) {
    assert.ok(kinds.includes(kind), `生成物里应声明深链 kind "${kind}"（M1 只接线这两条）`);
    assert.ok(
      modeWindow.includes(`"${kind}"`),
      `注入脚本里的深链 kind "${kind}" 必须与生成物一致`,
    );
  }
  for (const method of EVENT_SUBSCRIBED_METHODS) {
    assert.ok(names.includes(method), `${method} 必须在注入的 window.mode 上（否则 OAuth 链断）`);
  }

  // 入参归一化必须与 M1 的 `normalizeApplicationIconRequest` **逐字段一致**：
  // Rust 命令只收结构化 locator，字符串喂错形状会被反序列化拒掉（`getApplicationIcon` 直接报错）。
  // 早先版本只在注释里声称「两处由本用例对拍」而实际没有断言（review I2）——现在是断言。
  const sample = "com.example.app";
  await page.probe(`window.mode.getApplicationIcon(${JSON.stringify(sample)})`);
  await page.flushMicrotasks();
  const iconCall = page.invokes.find((call) => call.command === "getApplicationIcon");
  assert.ok(iconCall, "getApplicationIcon 必须真的走到壳命令");
  assert.deepEqual(
    JSON.parse(JSON.stringify(iconCall.args)),
    JSON.parse(JSON.stringify({ request: normalizeApplicationIconRequest(sample) })),
    "注入层的图标入参归一化必须与 M1 的 normalizeApplicationIconRequest 同形（两处口径分叉即红）",
  );
});

test("壳侧窗口控制面的 rename 字面量必须与注入名一致（漂移即红）", () => {
  // 为什么单独一条：`#[tauri::command(rename = "…")]` 只吃**字面量**，而注入层按名字 `invoke`，
  // 所以壳 crate 里必然有一份重复的命令名。两处漂移的症状是运行期 `Command … not found`
  // ——正是用户报过的「标题栏点了没反应」。本轮补充这条守卫之前，那两份字面量**没有任何
  // 东西在守**（注释里声称有守卫是假的，已改正）。
  //
  // 三处对拍：`window_controls.rs` 的 rename 字面量 ↔ `bridge.rs` 的 `CMD_*` 常量 ↔
  // 注入脚本里实际出现的名字（后者由上面那条用例按行为证）。
  const controlsSource = read(WINDOW_CONTROLS_RUST);
  // 命令名字面量按**标识符**收：注释里也会出现 `rename = "…"` 这种示意写法（本文件自己踩过），
  // 用 `[^"]+` 会把那个省略号当成一条命令。
  const renames = [
    ...controlsSource.matchAll(/#\[tauri::command\(rename = "([A-Za-z][A-Za-z0-9]*)"\)\]/g),
  ].map((match) => match[1]!);
  assert.ok(
    renames.length > 0,
    `${WINDOW_CONTROLS_RUST} 里应能读到 rename 字面量（读不到说明扫描规则失效了）`,
  );

  // `bridge.rs` 里被注入进脚本的命令名常量（按名字取，不看值）。
  const bridgeSource = read(BRIDGE_RUST);
  const constants = new Map(
    [...bridgeSource.matchAll(/pub const (CMD_\w+): &str = "([^"]+)"/g)].map((match) => [
      match[1]!,
      match[2]!,
    ]),
  );
  const windowControlConstants = [
    "CMD_EXECUTE_DESKTOP_COMMAND",
    "CMD_WINDOW_CHROME_STATE",
  ];
  for (const name of windowControlConstants) {
    assert.ok(
      constants.has(name),
      `${BRIDGE_RUST} 里应有 ${name}（注入层按它 invoke）`,
    );
  }

  // ① 壳注册的每个窗口控制命令都必须出现在 `bridge.rs` 的命令名常量里。
  for (const rename of renames) {
    assert.ok(
      [...constants.values()].includes(rename),
      `${WINDOW_CONTROLS_RUST} 注册了 "${rename}"，但 ${BRIDGE_RUST} 的 CMD_* 常量里没有它` +
        `（注入层不会 invoke 这个名字 → 那条命令永远收不到调用）`,
    );
  }
  // ② 反向：`bridge.rs` 声称注入会给这两个名字，壳就必须真的注册它们
  //    （少了就是「注入发得出去、壳不认」——同样是死按钮）。
  for (const name of windowControlConstants) {
    assert.ok(
      renames.includes(constants.get(name)!),
      `${BRIDGE_RUST} 的 ${name} 是 "${constants.get(name)}"，但 ${WINDOW_CONTROLS_RUST} 没有` +
        `用这个名字注册命令（漂移即运行期 Command not found）`,
    );
  }
});

test("窗口控制面（自绘标题栏）真的接上了壳：三个成员都在，且命令/状态/订阅都可观察", async () => {
  // 用户报的缺陷就是这里：这三个成员在 Tauri 壳里曾经是「降级空操作 / 缺席」，
  // 于是标题栏的三个按钮点了没反应。这条用例是它们的**回归网**：
  // ① 三个成员必须都注入（撤掉任一 → 这条红）；
  // ② `executeDesktopCommand` 必须真的把命令 id 发到壳（不是返回假值/空操作）；
  // ③ `getDesktopWindowChromeState` 必须回壳给的状态，形状不对时**拒绝**而不是把 undefined
  //    交给调用方画图标；
  // ④ `onDesktopWindowChromeStateChanged` 必须在状态真的变了时才回调（去重），并返回 disposer。
  const page = createPage();
  page.evaluate(emitInjectedScript());
  for (const name of WINDOW_CONTROL_METHODS) {
    // 注意：`page.probe` 返回的是**表达式的结果**，所以这里不能再套一层 `typeof`
    // （套了就变成 `typeof "function"`，恒为 "string"）。
    assert.equal(
      page.probe(`typeof window.mode.${name}`),
      "function",
      `${name} 必须注入（自绘标题栏依赖它；缺席即「点了没反应」）`,
    );
  }

  // ② 命令 id 原样发到壳：壳按 id 派发（窗口级的那些落到窗口上，其余带回 success:false）。
  page.probe("window.mode.executeDesktopCommand('minimizeWindow');");
  await page.flushMicrotasks();
  const executed = page.invokes.filter((call) => call.command === "executeDesktopCommand");
  assert.equal(
    executed.length,
    1,
    `executeDesktopCommand 必须真的走壳命令；本次记录到的调用：${JSON.stringify(page.invokes)}`,
  );
  assert.deepEqual(
    JSON.parse(JSON.stringify(executed[0]!.args)),
    { command: "minimizeWindow" },
    "命令 id 必须按壳的参数名（`command`）传，且原样不动",
  );

  // ③ 状态读取：壳给什么就回什么（不自己造 isMaximized）。
  const state = await page.probe("window.mode.getDesktopWindowChromeState()");
  assert.deepEqual(
    JSON.parse(JSON.stringify(state)),
    { isMaximized: false, macOSMajorVersion: null, supportsNativeRoundedCorners: false },
    "chrome 状态必须原样来自壳",
  );
});

test("窗口 chrome 状态订阅：只在状态真的变了时回调，且退订后不再回调", async () => {
  const page = createPage();
  page.evaluate(emitInjectedScript());
  // 壳侧的状态用可变的替身喂：先 false，切到 true 再读。
  page.probe(
    "window.__chrome = { isMaximized: false, macOSMajorVersion: null, supportsNativeRoundedCorners: false };" +
      "window.__windowApi = { resizedHandler: null };" +
      "window.__TAURI__ = window.__TAURI__ || {};" +
      "window.__TAURI__.window = {" +
      "  getCurrentWindow: function () {" +
      "    return {" +
      "      onResized: function (callback) { window.__windowApi.resizedHandler = callback; return Promise.resolve(function () {}); }" +
      "    };" +
      "  }" +
      "};" +
      "window.__seen = [];" +
      "window.__disposeChrome = window.mode.onDesktopWindowChromeStateChanged(function (s) { window.__seen.push(s.isMaximized); });",
  );
  await page.flushMicrotasks();

  // 订阅后立即读一次（调用方拿它当初值）：壳此刻报 false。
  assert.deepEqual(
    Array.from(page.probe("window.__seen") as boolean[]),
    [false],
    `订阅后必须立即给一次初值（标题栏靠它决定画哪个图标）；调用记录：${JSON.stringify(page.invokes)}，` +
      `降级：${JSON.stringify(page.probe("window.__MODE_WINDOW_MODE__.degraded"))}`,
  );

  // 状态没变 → 尺寸变化也不该再回调（去重）。
  page.probe("window.__windowApi.resizedHandler();");
  page.fireTimers();
  await page.flushMicrotasks();
  assert.deepEqual(
    Array.from(page.probe("window.__seen") as boolean[]),
    [false],
    "状态没变时不得重复回调（拖拽改尺寸会连发 resize 事件）",
  );

  // 状态真的变了（最大化）→ 回调一次。
  page.probe("window.__chrome.isMaximized = true; window.__windowApi.resizedHandler();");
  page.fireTimers();
  await page.flushMicrotasks();
  assert.deepEqual(
    Array.from(page.probe("window.__seen") as boolean[]),
    [false, true],
    "状态真的变了必须回调（否则最大化之后图标停在「最大化」上）",
  );

  // 退订：disposer 同步返回且此后不再回调。
  page.probe("window.__disposeChrome(); window.__chrome.isMaximized = false; window.__windowApi.resizedHandler();");
  page.fireTimers();
  await page.flushMicrotasks();
  assert.deepEqual(
    Array.from(page.probe("window.__seen") as boolean[]),
    [false, true],
    "退订后不得再回调",
  );
});

test("深链订阅：kind 与方法的配对是可观察行为，不是「两个字符串都出现过」", async () => {
  // 为什么单独一条（review I2）：原先只断言两个 kind 字面量**都出现在文件里**——
  // 把 `KIND_OAUTH` 与 `KIND_PAYMENT` 对调，那种断言照样绿。这里按行为验证：
  // 喂一个 kind，只有对应的方法该被回调。
  //
  // M3 Task 4 把订阅面从 2 条扩到 4 条（`share-import` / `workspace-open` 也有落点了），
  // 所以配对断言也扩到四条，并且**载荷形状各按各的**（share 是 `{shareCode}`、
  // workspace-open 是 `path`）——只断言「kind 出现过」抓不到投影写错。
  const page = createPage();
  page.evaluate(emitInjectedScript());
  page.probe(
    "window.__probe = { oauth: [], payment: [], share: [], workspace: [] };" +
      "window.mode.onOAuthCallback(function (url) { window.__probe.oauth.push(url); });" +
      "window.mode.onPaymentCallback(function (url) { window.__probe.payment.push(url); });" +
      "window.mode.onShareImport(function (payload) { window.__probe.share.push(payload); });" +
      "window.mode.onOpenWorkspacePath(function (path) { window.__probe.workspace.push(path); });",
  );
  await page.flushMicrotasks();

  const event = readDeepLinkEventName();
  assert.equal(page.deepLinkListeners.length, 4, "四条订阅方法都必须真的注册监听");
  for (const listener of page.deepLinkListeners) {
    assert.equal(listener.event, event, `订阅的必须是宿主 emit 的深链事件 ${event}`);
  }

  const deliver = (payload: Record<string, unknown>): void => {
    for (const listener of page.deepLinkListeners) {
      listener.handler({ event, payload });
    }
  };

  const recorded = (key: "oauth" | "payment" | "share" | "workspace"): unknown[] =>
    Array.from(page.probe(`window.__probe.${key}`) as unknown[]);
  /**
   * 跨 realm 比较：`share` 那一档的元素是**注入脚本 realm 里造的对象**，
   * 而 `node:vm` 的跨 realm 对象在原型上不等于本 realm 的字面量（deepStrictEqual 会红）。
   * 走一次 JSON 往返把形状（而不是原型）拿来比。
   */
  const recordedShape = (key: "oauth" | "payment" | "share" | "workspace"): unknown =>
    JSON.parse(JSON.stringify(recorded(key)));

  deliver({ kind: "oauth-callback", url: "mode://oauth/callback?state=oauth-1" });
  assert.deepEqual(
    recorded("oauth"),
    ["mode://oauth/callback?state=oauth-1"],
    "oauth-callback 必须打到 onOAuthCallback",
  );
  assert.deepEqual(
    recorded("payment"),
    [],
    "oauth-callback 不得打到 onPaymentCallback（两条方法对调过就会红）",
  );

  deliver({ kind: "payment-callback", url: "mode://pay/callback?state=pay-1" });
  assert.deepEqual(
    recorded("payment"),
    ["mode://pay/callback?state=pay-1"],
    "payment-callback 必须打到 onPaymentCallback",
  );
  assert.deepEqual(
    recorded("oauth"),
    ["mode://oauth/callback?state=oauth-1"],
    "payment-callback 不得追加到 onOAuthCallback",
  );

  // M3 Task 4 的两条：载荷形状**各自不同**，所以必须按各自的投影断言。
  deliver({ kind: "share-import", url: "mode://share/import?code=abc", code: "abc" });
  assert.deepEqual(
    recordedShape("share"),
    [{ shareCode: "abc" }],
    "share-import 必须打到 onShareImport，且载荷是 {shareCode}（不是把 code 直接透传）",
  );
  deliver({ kind: "workspace-open", url: "mode://workspace/open?path=/w", path: "/w" });
  assert.deepEqual(
    recorded("workspace"),
    ["/w"],
    "workspace-open 必须打到 onOpenWorkspacePath，且给的是解析后的 path",
  );
  assert.deepEqual(recorded("oauth"), ["mode://oauth/callback?state=oauth-1"], "别的订阅不得被追加");

  // 缺字段的两条：跳过，不喂半个载荷（回调拿到 `undefined` 会二次崩）。
  deliver({ kind: "share-import", url: "mode://share/import" } as Record<string, unknown>);
  deliver({ kind: "workspace-open", url: "mode://workspace/open" } as Record<string, unknown>);
  assert.deepEqual(recordedShape("share"), [{ shareCode: "abc" }]);
  assert.deepEqual(recorded("workspace"), ["/w"]);

  // 形状不对的载荷（IPC 来的东西不能假设类型）只跳过，不抛、也不误投。
  deliver({ kind: "oauth-callback" } as Record<string, unknown>);
  deliver({ kind: "oauth-callback", url: 42 } as unknown as Record<string, unknown>);
  deliver({ url: "mode://oauth/callback?state=no-kind" } as Record<string, unknown>);
  assert.deepEqual(
    recorded("oauth"),
    ["mode://oauth/callback?state=oauth-1"],
    "畸形载荷必须被跳过（否则一次畸形事件会打断整条订阅）",
  );
  assert.deepEqual(recorded("payment"), ["mode://pay/callback?state=pay-1"]);
});

test("必选成员要么是真实现、要么被拒 / 降级留痕；可选成员一律缺席（两处声明真源都守）", async () => {
  const page = createPage();
  page.evaluate(emitInjectedScript());
  const names = Array.from(page.probe("Object.keys(window.mode)") as string[]).sort();
  const declared = readWindowModeDeclaration();
  const declaredAll = new Set([...declared.required, ...declared.optional]);
  const platformService = readPlatformServiceDeclaration();
  const usage = readRendererWindowModeUsage();

  assert.equal(
    names.length,
    Object.keys(page.probe("window.mode") as object).length,
    "需要的键应当是可枚举的自有属性",
  );

  // 注入的成员名必须都能在**声明面**里找到依据——防止拼错一个名字（拼错即静默失效）。
  for (const name of names) {
    assert.ok(
      declaredAll.has(name) || platformService.has(name),
      `注入的 window.mode 出现了两处声明里都没有的成员 ${name}`,
    );
  }

  const implemented = new Set<string>([
    ...M1_IMPLEMENTED_COMMANDS,
    ...EVENT_SUBSCRIBED_METHODS,
    ...WINDOW_CONTROL_METHODS,
  ]);
  const absent = new Set(DELIBERATELY_ABSENT_REQUIRED_MEMBERS);
  assert.deepEqual(
    [...absent],
    DELIBERATELY_ABSENT_REQUIRED_MEMBERS,
    "「故意留空」的必选成员清单是显式决策，改它必须同时改这里与 spec",
  );

  // 声明的返回形态决定注入层给什么：要读值的 → 被拒 Promise；没有要读的值 → 降级空操作。
  const expectedThrowers = new Set<string>();
  const expectedNoOps = new Set<string>();
  const classify = (name: string, withoutConsumedValue: boolean): void => {
    (withoutConsumedValue ? expectedNoOps : expectedThrowers).add(name);
  };
  for (const name of declared.requiredThrowing) {
    if (!implemented.has(name) && !absent.has(name)) classify(name, false);
  }
  for (const name of declared.requiredWithoutConsumedValue) {
    if (!implemented.has(name) && !absent.has(name)) classify(name, true);
  }

  // 消费者侧的那批：渲染层**直接调用**但 `globals.d.ts` 没声明 —— 由 `IPlatformService` 定形态。
  // （注意：其中四个在 `IPlatformService` 里声明成可选，渲染层却**不加守卫**地调它们——
  //  这正是「覆盖规则必须跟着调用点走」的原因，见 review I1。）
  const consumerOnly = usage.unsafeCalls.filter((name) => !declaredAll.has(name));
  assert.ok(
    consumerOnly.length > 0,
    "应当存在「渲染层直接调用、但 globals.d.ts 未声明」的成员（否则这段断言是空的）",
  );
  for (const name of consumerOnly) {
    const withoutConsumedValue = platformService.get(name);
    assert.ok(
      withoutConsumedValue !== undefined,
      `${name} 被渲染层直接调用，但 globals.d.ts 与 IPlatformService 都没声明它——` +
        `形态未知就不能猜，必须先确定它的返回形态`,
    );
    classify(name, withoutConsumedValue);
  }

  // 可选成员不得被塞桩：调用方用 `?.` 守卫，「缺席」才是正确形态。
  for (const name of declared.optional) {
    assert.ok(
      !names.includes(name) || implemented.has(name),
      `可选成员 ${name} 不得被注入（缺席才是「平台不支持」的正确形态；唯一例外是 M1 已实现的切片）`,
    );
  }

  // ① 要读值的必选成员：调用必须返回一个**带方法名的被拒 Promise**——不返回假值，
  //    也不同步抛（实测：渲染期同步抛会掀翻整棵树，见 spec §10.9/§10.10）。
  for (const name of expectedThrowers) {
    assert.ok(names.includes(name), `未实现的必选成员 ${name} 必须注入（不静默缺席）`);
    assert.equal(
      page.probe(`typeof window.mode.${name}`),
      "function",
      `${name} 桩必须是函数（读它不得抛）`,
    );
    assert.equal(
      page.probe(
        `(function () {
           try {
             var returned = window.mode.${name}();
             // 吸收了拒绝，免得在这一步制造一条 unhandledRejection（下一条断言会专门读拒绝消息）。
             if (returned && typeof returned.then === "function") {
               returned.then(function () {}, function () {});
             }
             return "no-throw";
           } catch (error) {
             return "sync-throw:" + String(error && error.message);
           }
         })()`,
      ),
      "no-throw",
      `${name} 不得**同步**抛：渲染层在渲染期就会调这类方法，同步抛会变成一次 render 错误`,
    );
    const rejected = (await page.probe(
      `window.mode.${name}().then(function () { return "resolved"; }, function (error) { return String(error && error.message); })`,
    )) as string;
    assert.equal(
      rejected,
      `not_implemented_until_m3: ${name}`,
      `${name} 的拒绝必须带方法名（M1 口径：失败点要能定位，不是 undefined is not a function）`,
    );
    const degraded = Array.from(page.probe("window.__MODE_WINDOW_MODE__.degraded") as string[]);
    assert.ok(
      degraded.includes(`not_implemented_rejected:${name}`),
      `${name} 的拒绝必须留痕：${degraded.join(", ")}`,
    );
  }

  // ② 没有要读的值的必选成员：降级空操作，且三种消费形态都要成立、且**必须留痕**。
  for (const name of expectedNoOps) {
    assert.ok(names.includes(name), `未实现的必选成员 ${name} 必须注入（渲染层会不加守卫地调它）`);
    const probe = page.probe(
      `(function () {
         var result = window.mode.${name}();
         var callable = typeof result === "function";
         var disposable = false;
         try { result(); disposable = true; } catch (error) { disposable = String(error); }
         if (typeof result.then !== "function") {
           return { callable: callable, disposable: disposable, thenable: false };
         }
         return { callable: callable, disposable: disposable, thenable: true };
       })()`,
    ) as { callable: boolean; disposable: unknown; thenable: boolean };
    assert.equal(probe.callable, true, `${name} 的降级返回值必须可调用（订阅型调用方拿它当 disposer）`);
    assert.equal(probe.disposable, true, `${name} 的降级 disposer 被调用时不得抛`);
    assert.equal(probe.thenable, true, `${name} 的降级返回值必须可 await（通知型调用方会 await/catch）`);
    await page.probe(`window.mode.${name}()`);
    await page.flushMicrotasks();
    const degraded = Array.from(page.probe("window.__MODE_WINDOW_MODE__.degraded") as string[]);
    assert.ok(
      degraded.includes(`not_implemented_noop:${name}`),
      `${name} 的降级必须留痕（degraded），不能静默装成成功：${degraded.join(", ")}`,
    );
  }

  // ③ 分类必须**只由声明形态决定**：注入的两类集合要机械算出的两类一一对上。
  //    `Array.from`：注入脚本在另一个 realm（`node:vm`），跨 realm 的数组直接 deepStrictEqual
  //    会在原型上不等（内容一样也红）。
  const injectedNoOps = Array.from(page.probe("window.__MODE_WINDOW_MODE__.noOp") as string[]).sort();
  const injectedThrowers = Array.from(
    page.probe("window.__MODE_WINDOW_MODE__.notImplemented") as string[],
  ).sort();
  assert.deepEqual(
    injectedNoOps,
    [...expectedNoOps].sort(),
    "空操作成员集必须等于「声明没有要读的值」那一类",
  );
  assert.deepEqual(
    injectedThrowers,
    [...expectedThrowers].sort(),
    "被拒成员集必须等于「声明会返回被消费的值」那一类",
  );

  // ④ M1 自己的「未实现必选」清单里，凡是 window.mode 也声明的名字，都必须被注入
  //    （两套接口面交叉的部分不能有沉默缺口）——要么被拒、要么降级，但都要在。
  const intersection = M1_UNIMPLEMENTED_REQUIRED_METHODS.filter((name) =>
    declared.required.includes(name),
  );
  assert.ok(intersection.length > 0, "两套接口面必有交叉，否则这条断言是空的");
  for (const name of intersection) {
    assert.ok(
      expectedThrowers.has(name) || expectedNoOps.has(name),
      `${name} 在 M1 里也是未实现的必选方法，必须被注入`,
    );
  }
});
