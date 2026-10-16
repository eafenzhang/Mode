/**
 * M3 Task 3 · Step 2：`window.mode` 注入的**顺序性质**与**切片一致性**（进 CI：
 * `pnpm --filter @mode/desktop-tauri test`）。
 *
 * ## 为什么这条用例必须存在（DR-T3-1 的代价）
 *
 * Tauri 没有 preload / `contextBridge`，所以 renderer 启动时读的 `window.mode` 只能由**页面加载前**
 * 的注入提供。`packages/desktop/src/renderer/src/main.tsx` 在**模块求值期**就建桌面平台服务
 * （`const desktopPlatform = createDesktopPlatform({...})`，顶层语句），而 `createDesktopPlatform`
 * 返回的对象字面量里有若干**非惰性**的 `window.mode.X ? … : undefined` 条件表达式——也就是说
 * `window.mode` 缺席时，renderer 的入口模块**当场抛**，React 永远挂不上。
 * 「注入早于 renderer 第一次访问」由此成为**正确性的一部分**，不能靠「一般是够快的」。
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
 * 清单与 `.replace(…)` 清单派生（那两处是注入的真源），所以「新加一个模板忘了代入」或
 * 「加了模板但没被注入」都会当场红。
 */
import { readFileSync } from "node:fs";
import { createContext, runInContext } from "node:vm";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import assert from "node:assert/strict";
import test from "node:test";
import ts from "typescript";

import {
  M1_EVENT_SUBSCRIBED_METHODS,
  M1_IMPLEMENTED_COMMANDS,
  M1_UNIMPLEMENTED_REQUIRED_METHODS,
} from "@mode/ui/tauri-platform";

const here = dirname(fileURLToPath(import.meta.url));
const repoRoot = resolve(here, "../../..");
const read = (relativePath: string): string =>
  readFileSync(resolve(repoRoot, relativePath), "utf8");

const BRIDGE_RUST = "crates/sidecar-host/src/bridge.rs";
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
  for (const match of production.matchAll(/pub const (CMD_\w+): &str = "([a-z_]+)"/g)) {
    consts.set(match[1]!, match[2]!);
  }

  const placeholders = [...production.matchAll(/\.replace\("(__MODE_\w+__)",\s*([^)]+)\)/g)].map(
    (match) => {
      const placeholder = match[1]!;
      const expression = match[2]!.trim();
      if (expression.includes("safe_id")) {
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
  /** 读一个全局表达式（探针用法：先读，后断言）。 */
  probe(expression: string): unknown;
  fireDomContentLoaded(): void;
  flushMicrotasks(): Promise<void>;
  postMessages: Array<{ payload: unknown; transferred: unknown[] }>;
  invokes: Array<{ command: string; args: unknown }>;
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
  sandbox.MessageChannel = function MessageChannelStub() {
    return { port1: new FakePort(), port2: new FakePort() };
  };
  sandbox.CustomEvent = CustomEvent;
  sandbox.setTimeout = (callback: () => void) => {
    // 测试里不让定时器自己跑：投递时序要能被显式控制，否则断言变成「等一会儿再看」。
    void callback;
    return 0;
  };
  sandbox.clearTimeout = () => {};
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
    return Promise.resolve(null);
  };
  sandbox.__TAURI_INTERNALS__ = { invoke: recordInvoke };
  sandbox.__TAURI__ = {
    core: { invoke: recordInvoke },
    event: { listen: () => Promise.resolve(() => {}) },
  };

  const context = createContext(sandbox);
  return {
    evaluate: (script: string) => {
      runInContext(script, context);
    },
    probe: (expression: string) => runInContext(expression, context),
    fireDomContentLoaded: () => {
      (sandbox.document as { readyState: string }).readyState = "complete";
      for (const handler of domContentLoaded.splice(0)) handler();
    },
    flushMicrotasks: () => new Promise((resolveFlush) => setImmediate(resolveFlush)),
    postMessages,
    invokes,
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
  for (const method of M1_EVENT_SUBSCRIBED_METHODS) {
    assert.ok(names.includes(method), `${method} 必须在注入的 window.mode 上（否则 OAuth 链断）`);
  }
});

test("必选成员要么是真实现、要么显式抛 / 降级留痕；可选成员一律缺席", async () => {
  const page = createPage();
  page.evaluate(emitInjectedScript());
  const names = page.probe("Object.keys(window.mode).sort()") as string[];
  const declared = readWindowModeDeclaration();
  const declaredAll = new Set([...declared.required, ...declared.optional]);

  assert.equal(
    names.length,
    Object.keys(page.probe("window.mode") as object).length,
    "需要的键应当是可枚举的自有属性",
  );

  // 注入的成员名必须都是 window.mode 类型里声明过的——防止拼错一个名字（拼错即静默失效）。
  for (const name of names) {
    assert.ok(declaredAll.has(name), `注入的 window.mode 出现了未声明的成员 ${name}`);
  }

  const implemented = new Set<string>([...M1_IMPLEMENTED_COMMANDS, ...M1_EVENT_SUBSCRIBED_METHODS]);
  const absent = new Set(DELIBERATELY_ABSENT_REQUIRED_MEMBERS);
  // 声明的返回形态决定注入层该给什么：要读值的 → 显式抛；没有要读的值 → 降级空操作。
  const expectedThrowers = declared.requiredThrowing.filter(
    (name) => !implemented.has(name) && !absent.has(name),
  );
  const expectedNoOps = declared.requiredWithoutConsumedValue.filter(
    (name) => !implemented.has(name) && !absent.has(name),
  );
  assert.ok(expectedThrowers.length > 0 && expectedNoOps.length > 0, "两类桩都不该为空");

  // 可选成员不得被塞桩：调用方用 `?.` 守卫，「缺席」才是正确形态。
  for (const name of declared.optional) {
    assert.ok(
      !names.includes(name) || implemented.has(name),
      `可选成员 ${name} 不得被注入（缺席才是「平台不支持」的正确形态；唯一例外是 M1 已实现的切片）`,
    );
  }
  assert.deepEqual(
    [...absent],
    DELIBERATELY_ABSENT_REQUIRED_MEMBERS,
    "「故意留空」的必选成员清单是显式决策，改它必须同时改这里与 spec",
  );

  // ① 要读值的必选成员：调用必须返回一个**带方法名的被拒 Promise**——不返回假值，
  //    也不同步抛（实测：渲染期同步抛会掀翻整棵树，见 spec §10.8/§10.9）。
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
    const degraded = Array.from(
      page.probe("window.__MODE_WINDOW_MODE__.degraded") as string[],
    );
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
           return { callable: callable, disposable: disposable, thenable: false, viaCall: null };
         }
         return { callable: callable, disposable: disposable, thenable: true };
       })()`,
    ) as { callable: boolean; disposable: unknown; thenable: boolean };
    assert.equal(probe.callable, true, `${name} 的降级返回值必须可调用（订阅型调用方拿它当 disposer）`);
    assert.equal(probe.disposable, true, `${name} 的降级 disposer 被调用时不得抛`);
    assert.equal(probe.thenable, true, `${name} 的降级返回值必须可 await（通知型调用方会 await/catch）`);
    await page.probe(`window.mode.${name}()`);
    await page.flushMicrotasks();
    const degraded = page.probe("window.__MODE_WINDOW_MODE__.degraded") as string[];
    assert.ok(
      degraded.includes(`not_implemented_noop:${name}`),
      `${name} 的降级必须留痕（degraded），不能静默装成成功：${degraded.join(", ")}`,
    );
  }

  // ③ 分类必须**只由声明形态决定**：注入的桩集合要与上面机械算出的两类一一对上。
  // `Array.from`：注入脚本在另一个 realm（`node:vm`），跨 realm 的数组直接 deepStrictEqual
  // 会在原型上不等（内容一样也红）。
  const injectedNoOps = Array.from(
    page.probe("window.__MODE_WINDOW_MODE__.noOp") as string[],
  ).sort();
  const injectedThrowers = Array.from(
    page.probe("window.__MODE_WINDOW_MODE__.notImplemented") as string[],
  ).sort();
  assert.deepEqual(injectedNoOps, [...expectedNoOps].sort(), "空操作成员集必须等于「声明没有要读的值」那一类");
  assert.deepEqual(
    injectedThrowers,
    [...expectedThrowers].sort(),
    "抛错成员集必须等于「声明会返回被消费的值」那一类",
  );

  // ④ M1 自己的「未实现必选」清单里，凡是 window.mode 也声明的名字，都必须被注入
  //    （两套接口面交叉的部分不能有沉默缺口）——要么抛、要么降级，但都要在。
  const intersection = M1_UNIMPLEMENTED_REQUIRED_METHODS.filter((name) =>
    declared.required.includes(name),
  );
  assert.ok(intersection.length > 0, "两套接口面必有交叉，否则这条断言是空的");
  for (const name of intersection) {
    assert.ok(
      expectedThrowers.includes(name) || expectedNoOps.includes(name),
      `${name} 在 M1 里也是未实现的必选方法，必须被注入`,
    );
  }
});
