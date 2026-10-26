/**
 * M1 平台命令面的**验收台**（规格 `docs/specs/tauri-platform-commands.md` §4）。
 *
 * 它把 spec §4 的验收表变成可点的东西：10 条命令各一行，能填参、能跑、能看结果与错误；
 * 深链回调单开一块，把宿主 emit 的事件打出来——这样「OAuth 回调通道」这条 M1 唯一接线的
 * 事件链也有肉眼可见的验证方式（配合 `mode://` 链接或命令行传参）。
 *
 * 表格是**唯一真源**：命令名、入参、默认值、调用方式都写在 `COMMAND_SPECS` 里，
 * 渲染与调用都从它派生。命令名类型取自适配层导出的切片表，漏一条或写错名字会在
 * typecheck 阶段暴露。
 */
import { M1_IMPLEMENTED_COMMANDS } from "@mode/ui/tauri-platform";

import type { FrontendPlatform } from "./platform.js";

/** 命令切片表里的命令名。 */
type M1Command = (typeof M1_IMPLEMENTED_COMMANDS)[number];

/** 一行命令的入参描述。 */
type FieldSpec = {
  key: string;
  label: string;
  /** 预填值（空串表示留空）。 */
  initial: string;
  /** 输入控件类型；`json` 是单行 JSON，提交前解析。 */
  kind: "text" | "number" | "json";
};

/** 一行命令。 */
type CommandSpec = {
  name: M1Command;
  summary: string;
  fields: FieldSpec[];
  /** 由入参构造调用。返回 Promise 或值都可以。 */
  call: (platform: FrontendPlatform, values: Record<string, string>) => unknown;
};

/** 把逗号分隔的输入切成数组，去掉空项。 */
function splitList(value: string): string[] {
  return value
    .split(",")
    .map((item) => item.trim())
    .filter((item) => item.length > 0);
}

/**
 * spec §4 验收表的可执行版本。
 *
 * 用 `satisfies` 而不是 `: CommandSpec[]` 标注：类型标注会把 `name` 拓宽成整个联合，
 * 下面的覆盖守卫就永远成立（变成空守卫）。`satisfies` 保留字面量类型，
 * 守卫才真的能发现漏项。
 */
const COMMAND_SPECS = [
  {
    name: "openExternal",
    summary: "交给系统默认处理器打开 URL",
    fields: [{ key: "url", label: "url", initial: "https://example.test/", kind: "text" }],
    call: (platform, values) => platform.openExternal?.(values.url ?? ""),
  },
  {
    name: "selectDirectory",
    summary: "目录选择框；取消返回 null",
    fields: [],
    call: (platform) => platform.selectDirectory?.(),
  },
  {
    name: "selectFile",
    summary: "文件选择框；取消返回 null",
    fields: [],
    call: (platform) => platform.selectFile?.(),
  },
  {
    name: "saveFile",
    summary: "写盘（data 分支）；取消返回 canceled: true",
    fields: [
      { key: "suggestedName", label: "suggestedName", initial: "mode-skeleton.txt", kind: "text" },
      { key: "content", label: "内容", initial: "hello from Tauri skeleton\n", kind: "text" },
    ],
    call: (platform, values) =>
      platform.saveFile?.({
        suggestedName: values.suggestedName ?? "mode-skeleton.txt",
        data: new TextEncoder().encode(values.content ?? "").buffer as ArrayBuffer,
      } as never),
  },
  {
    name: "saveFile",
    summary: "写盘（sourceUrl 分支）；取消返回 canceled: true",
    fields: [
      { key: "suggestedName", label: "suggestedName", initial: "mode-source.txt", kind: "text" },
      {
        key: "sourceUrl",
        label: "sourceUrl",
        initial: "http://127.0.0.1:8765/payload.txt",
        kind: "text",
      },
    ],
    call: (platform, values) =>
      platform.saveFile?.({
        suggestedName: values.suggestedName ?? "mode-source.txt",
        sourceUrl: values.sourceUrl ?? "",
      } as never),
  },
  {
    name: "createTempTextAttachment",
    summary: "长粘贴文本 → 临时附件；返回 filename/localPath/mimeType/sizeBytes",
    fields: [
      { key: "text", label: "文本", initial: "hello from Tauri skeleton\n", kind: "text" },
      { key: "filename", label: "filename", initial: "paste.txt", kind: "text" },
    ],
    call: (platform, values) =>
      platform.createTempTextAttachment?.({
        text: values.text ?? "",
        ...(values.filename ? { filename: values.filename } : {}),
      }),
  },
  {
    name: "openInFileManager",
    summary: "目录→打开；文件→在父目录里选中；不存在→success:false",
    fields: [{ key: "path", label: "path", initial: "", kind: "text" }],
    call: (platform, values) => platform.openInFileManager?.(values.path ?? ""),
  },
  {
    name: "openExternalFile",
    summary: "用系统默认应用打开本地文件；目录/不存在→success:false",
    fields: [{ key: "path", label: "path", initial: "", kind: "text" }],
    call: (platform, values) => platform.openExternalFile?.(values.path ?? ""),
  },
  {
    name: "showTaskNotification",
    summary: "系统通知（是否展示由系统决定）",
    fields: [
      { key: "title", label: "标题", initial: "Mode 骨架", kind: "text" },
      { key: "body", label: "正文", initial: "来自 Tauri 平台命令面", kind: "text" },
    ],
    call: (platform, values) =>
      platform.showTaskNotification?.({
        taskId: "skeleton-task",
        status: "completed",
        title: values.title ?? "",
        body: values.body ?? "",
      }),
  },
  {
    name: "notifyRendererReady",
    summary: "渲染层握手：宿主收到后补发积压的冷启动深链",
    fields: [],
    call: (platform) => platform.notifyRendererReady?.(),
  },
  {
    name: "syncWindowTabs",
    summary: "把当前窗口的 tab 路径同步给宿主（逗号分隔）",
    fields: [{ key: "paths", label: "paths", initial: "D:/ws/a,D:/ws/b", kind: "text" }],
    call: (platform, values) => platform.syncWindowTabs?.(splitList(values.paths ?? "")),
  },
  {
    name: "syncWindowUnreadCount",
    summary: "未读 task 数",
    fields: [{ key: "count", label: "count", initial: "3", kind: "number" }],
    call: (platform, values) => platform.syncWindowUnreadCount?.(Number(values.count ?? 0)),
  },
  {
    name: "getApplicationIcon",
    summary: "M1 恒返回 null（原生图标解码登记在 M3）",
    fields: [{ key: "appId", label: "应用标识", initial: "", kind: "text" }],
    call: (platform, values) => platform.getApplicationIcon?.(values.appId ?? ""),
  },
  {
    name: "rdReadClipboardText",
    summary: "LR-3：读系统剪贴板文本（Windows CF_UNICODETEXT；无文本为空串）",
    fields: [],
    call: (platform) => platform.rdReadClipboardText?.(),
  },
  {
    name: "syncAppSettings",
    summary:
      "窗口行为类设置同步（壳唯一执行者）：closeToTrayOnWindows=true 且托盘可用时，关闭窗口改为隐藏到托盘",
    fields: [
      {
        key: "closeToTrayOnWindows",
        label: "closeToTrayOnWindows",
        initial: "true",
        kind: "text",
      },
    ],
    call: (platform, values) =>
      platform.syncAppSettings?.({
        closeToTrayOnWindows: (values.closeToTrayOnWindows ?? "true").trim() === "true",
      }),
  },
  {
    name: "exportLogs",
    summary:
      "LR-5 §27：Host 文件日志导出成单文件文本包（保存对话框；取消 = success:false 且无 error）",
    fields: [],
    call: (platform) => platform.exportLogs?.(),
  },
] satisfies CommandSpec[];

/**
 * 编译期守卫：`COMMAND_SPECS` 必须覆盖切片表里的每一条命令。
 *
 * 反向（表里出现切片外的名字）由 `CommandSpec["name"]` 的类型挡住；这里挡的是漏项——
 * 切片新增一条命令而验收台没跟上时，`pnpm typecheck` 直接报错，而不是让人以为
 * 「验收台跑绿了 = 命令面都验过了」。
 */
const _coversEveryCommand: Exclude<M1Command, (typeof COMMAND_SPECS)[number]["name"]> extends never
  ? true
  : never = true;
void _coversEveryCommand;

/** 把任意返回值转成可读文本。 */
function formatResult(value: unknown): string {
  if (value === undefined) return "(void)";
  try {
    return JSON.stringify(value, null, 2) ?? String(value);
  } catch {
    return String(value);
  }
}

function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className?: string,
  text?: string,
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

/** 渲染验收台。`platform` 由 `platform.ts` 装配。 */
export function renderHarness(root: HTMLElement, platform: FrontendPlatform): void {
  root.replaceChildren();

  const header = el("header");
  header.append(el("h1", undefined, "Mode — Tauri 平台命令面（M1 骨架）"));
  header.append(
    el(
      "p",
      "hint",
      "本页是平台命令面的验收台，不是产品 UI。产品 UI 的挂载需要 M3 的 Node sidecar（Host），见 docs/specs/tauri-platform-commands.md §5。",
    ),
  );
  root.append(header);

  const list = el("section", "commands");
  for (const spec of COMMAND_SPECS) {
    list.append(renderCommandRow(spec, platform));
  }
  root.append(list);

  root.append(renderDeepLinkPanel(platform));
}

function renderCommandRow(spec: CommandSpec, platform: FrontendPlatform): HTMLElement {
  const row = el("article", "command");
  const head = el("div", "command-head");
  head.append(el("code", "command-name", spec.name));
  head.append(el("span", "command-summary", spec.summary));
  row.append(head);

  const inputs = new Map<string, HTMLInputElement>();
  const values: Record<string, string> = {};
  if (spec.fields.length > 0) {
    const fields = el("div", "fields");
    for (const field of spec.fields) {
      const label = el("label", "field");
      label.append(el("span", "field-label", field.label));
      const input = document.createElement("input");
      input.type = field.kind === "number" ? "number" : "text";
      input.value = field.initial;
      input.spellcheck = false;
      label.append(input);
      fields.append(label);
      inputs.set(field.key, input);
      values[field.key] = field.initial;
    }
    row.append(fields);
  }

  const output = el("pre", "result");
  const run = el("button", "run", "运行");
  run.type = "button";
  run.addEventListener("click", () => {
    for (const [key, input] of inputs) values[key] = input.value;
    output.className = "result";
    output.textContent = "…";
    void (async () => {
      try {
        const result = await spec.call(platform, values);
        output.textContent = formatResult(result);
      } catch (error) {
        // 失败**原样显示**，不吞：spec §4 的判据里「抛错」本身就是一种失败信号。
        output.className = "result result-error";
        output.textContent =
          error instanceof Error ? `${error.name}: ${error.message}` : String(error);
      }
    })();
  });

  const actions = el("div", "actions");
  actions.append(run);
  row.append(actions);
  row.append(output);
  return row;
}

/**
 * 深链回调面板。
 *
 * 订阅 `onOAuthCallback` / `onPaymentCallback`，把收到的 URL 追加到日志里。
 * 这里同时是**握手顺序**的可视化：先订阅、再 `notifyRendererReady`，与适配层的约定一致。
 */
function renderDeepLinkPanel(platform: FrontendPlatform): HTMLElement {
  const panel = el("section", "deeplink");
  panel.append(el("h2", undefined, "深链回调（M1 唯一接线的事件通道）"));
  panel.append(
    el(
      "p",
      "hint",
      "订阅已装好。用 mode://oauth/callback?state=test 打开本应用（或带该参数重启）时应出现在下面。",
    ),
  );

  const log = el("pre", "result");
  log.textContent = "(尚无回调)";
  const received: string[] = [];
  const append = (kind: string, url: string) => {
    received.push(`${kind}  ${url}`);
    log.textContent = received.join("\n");
  };

  platform.onOAuthCallback?.((url) => append("oauth-callback", url));
  platform.onPaymentCallback?.((url) => append("payment-callback", url));

  // 握手：适配层保证这一步会等订阅装好之后才发（否则宿主补发的冷启动深链没人接）。
  platform.notifyRendererReady?.();

  panel.append(log);
  return panel;
}
