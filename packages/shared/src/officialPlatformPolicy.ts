/** 审计版：官方平台断连策略。
 *
 * 官方平台服务已整体下线：没有开关、没有设置入口、没有环境变量，任何进程都不得发起官方平台请求
 * （Z.AI 市场、客户端配置、账号/套餐等）。凭证读取与网络请求前一律短路。
 *
 * 保留两类 key 只为让调用点与文案说清「为什么不可用」：
 * - 对话分享：早已整体下线；
 * - account / codingPlan / officialMcp / offPeak：随「去智谱化」把智谱套餐与
 *   官方账号体系一并移除，能力已不存在，只保留 key 让调用点与文案能说清「已下线」而不是含糊拒绝。
 */

/** 平台路径表里登记的官方功能键；已无开关，恒不可用。 */
export type OfficialServiceKey = "marketplace" | "clientConfig";

/** 随去智谱化移除的官方功能：没有开关，恒定不可用。 */
export type RemovedOfficialServiceKey =
  | "account"
  | "codingPlan"
  | "officialMcp"
  | "offPeak";

/**
 * 官方平台服务已整体下线：不再有开关，也不再发起任何官方平台请求
 * （Z.AI 市场、客户端配置、账号/套餐等）。保留同签名函数让调用点保持短路语义。
 */
export function isOfficialServiceEnabled(_key: OfficialServiceKey): boolean {
  return false;
}

export function assertOfficialServiceAvailable(_key: OfficialServiceKey): void {
  throw new Error(`官方平台服务已下线，不再连接 Z.AI；反馈请访问 ${MODE_ISSUES_URL}`);
}

/** 对话分享已下线：不提供开关，任何组合都不能恢复。 */
export function isConversationShareAvailable(): boolean {
  return false;
}

export function assertConversationShareRemoved(): void {
  throw new Error(`对话分享已在 Mode 下线。反馈请访问 ${MODE_ISSUES_URL}`);
}

export const MODE_ISSUES_URL = "https://github.com/eafenzhang/Mode/issues";

/**
 * 已下线功能的中文名：只用于报错文案。
 * 报错不能直接印英文 key——用户看到 "codingPlan 未开启" 无从判断是什么功能，
 * 而这批能力在设置页里原本显示的就是这些中文名。
 */
const REMOVED_OFFICIAL_SERVICE_LABELS: Readonly<Record<RemovedOfficialServiceKey, string>> = {
  account: "官方账号登录",
  codingPlan: "官方套餐与额度",
  officialMcp: "官方 MCP 凭证",
  offPeak: "官方闲时任务",
};

function isRemovedOfficialServiceKey(
  key: OfficialServiceKey | RemovedOfficialServiceKey,
): key is RemovedOfficialServiceKey {
  return key in REMOVED_OFFICIAL_SERVICE_LABELS;
}

/**
 * 该功能是否已下线。
 *
 * 仍需保留 dormant 代码的调用点用它做前置短路，而不是直接写无条件 return：
 * 无条件 return 会让后续 dormant 代码整体落入 TypeScript 的不可达区，而 TS 在不可达区
 * 不做类型收窄，会把整段账号/套餐流程打成上百个类型错误。账号子系统的物理删除另开 issue 跟踪，
 * 本轮只摘掉开关依赖、保留代码形状，因此这里保留条件判断的写法。
 */
export function isOfficialServiceRemoved(key: OfficialServiceKey | RemovedOfficialServiceKey): boolean {
  return isRemovedOfficialServiceKey(key);
}

/** 已下线功能：恒抛异常，与对话分享同构——不存在“开关打开后恢复”的路径。 */
export function assertOfficialServiceRemoved(key: RemovedOfficialServiceKey): void {
  throw new Error(
    `“${REMOVED_OFFICIAL_SERVICE_LABELS[key]}”功能已在 Mode 下线，无法再开启。反馈请访问 ${MODE_ISSUES_URL}`,
  );
}

export function assertOfficialPlatformAvailable(): void {
  throw new Error(`官方平台服务已下线，不再连接 Z.AI；反馈请访问 ${MODE_ISSUES_URL}`);
}

/** 仅阻断平台域名，保留用户自配的模型 API、代理和本地地址。 */
export function isOfficialPlatformUrl(input: string | URL): boolean {
  try {
    const host = new URL(String(input)).hostname.toLowerCase().replace(/\.$/, "");
    return ["zcode.z.ai", "cdn-zcode.z.ai"].some(
      (domain) => host === domain || host.endsWith(`.${domain}`),
    );
  } catch {
    return false;
  }
}

/** 官方平台上的业务路径归类：仅用于把拦截理由精确指向具体功能，全部恒被拦截。 */
const OFFICIAL_SERVICE_PATH_RULES: ReadonlyArray<{
  key: OfficialServiceKey;
  patterns: readonly RegExp[];
}> = [
  {
    key: "marketplace",
    patterns: [/^\/api\/v1\/(plugin|marketplace|scenes|preview|models)/, /^\/deps\//],
  },
  {
    key: "clientConfig",
    patterns: [
      /^\/api\/v1\/(client|configs|bootstrap|event|releases|manifest|report|remote-control|server-info)/,
    ],
  },
];

/**
 * 已下线功能的原路径规则，正则原样保留。
 * 删掉它们会让这些 URL 落到“已下线或未登记”的含糊文案，调用方无法分辨是下线还是没登记；
 * 保留则是为了让拦截理由精确指向具体功能。
 */
const REMOVED_OFFICIAL_SERVICE_PATH_RULES: ReadonlyArray<{
  key: RemovedOfficialServiceKey;
  patterns: readonly RegExp[];
}> = [
  {
    key: "account",
    patterns: [
      /^\/api\/v1\/(oauth|login|logout|token|authorize|user|users|account|customer|organization|client\/claim)/,
    ],
  },
  {
    key: "codingPlan",
    patterns: [
      /^\/api\/v1\/(coding-plan|subscription|balance|billing|order|orders|claim|enterprise|pay|usage|zcode-plan)/,
    ],
  },
  { key: "officialMcp", patterns: [/^\/api\/v1\/mcp/] },
  { key: "offPeak", patterns: [/^\/api\/v1\/off-peak/, /\/off-peak/] },
];

/**
 * 官方 URL 归属的功能键；返回 null 表示不归属任何登记功能——分享与未知路径
 * 一律拒绝，登记路径同样一律拒绝，区分只为让报错指向具体功能。
 *
 * 已下线表必须先匹配：account 的 `client/claim` 会被 clientConfig 的 `client` 前缀吞掉，
 * 若按“先保留表后已移除表”匹配，这条套餐认领路径会被误报成 clientConfig。
 * 两表其余路径互不重叠。
 */
export function resolveOfficialServiceForUrl(
  input: string | URL,
): OfficialServiceKey | RemovedOfficialServiceKey | null {
  let url: URL;
  try {
    url = new URL(String(input));
  } catch {
    return null;
  }
  const host = url.hostname.toLowerCase().replace(/\.$/, "");
  const isCdn = host === "cdn-zcode.z.ai" || host.endsWith(".cdn-zcode.z.ai");
  const isPlatform = host === "zcode.z.ai" || host.endsWith(".zcode.z.ai");
  if (!isCdn && !isPlatform) {
    return null;
  }
  if (isCdn) {
    return "marketplace";
  }
  const path = url.pathname.toLowerCase();
  // 对话分享已下线：无论开关如何都拒绝。
  if (/(^|\/)share(\/|$)/.test(path)) {
    return null;
  }
  for (const rule of REMOVED_OFFICIAL_SERVICE_PATH_RULES) {
    if (rule.patterns.some((pattern) => pattern.test(path))) {
      return rule.key;
    }
  }
  for (const rule of OFFICIAL_SERVICE_PATH_RULES) {
    if (rule.patterns.some((pattern) => pattern.test(path))) {
      return rule.key;
    }
  }
  return null;
}

/**
 * 出口拦截判断：非平台 URL 放行；平台 URL 一律拦截——已下线功能、已登记功能与未登记路径
 * 都没有可放行的开关，保留功能键划分只为让报错说明是哪个功能不可用。
 */
export function shouldBlockOfficialPlatformUrl(input: string | URL): boolean {
  if (!isOfficialPlatformUrl(input)) {
    return false;
  }
  const key = resolveOfficialServiceForUrl(input);
  if (key === null) {
    return true;
  }
  if (isRemovedOfficialServiceKey(key)) {
    return true;
  }
  return !isOfficialServiceEnabled(key);
}

export function assertNoOfficialPlatformUrl(input: string | URL): void {
  if (isOfficialPlatformUrl(input)) assertOfficialPlatformAvailable();
}

/** 出口断言：官方 URL 一律拒绝，报错按功能键区分「已下线」与「平台整体下线」。 */
export function assertOfficialPlatformAccessible(input: string | URL): void {
  if (!shouldBlockOfficialPlatformUrl(input)) {
    return;
  }
  const key = resolveOfficialServiceForUrl(input);
  if (key !== null && isRemovedOfficialServiceKey(key)) {
    throw new Error(
      `“${REMOVED_OFFICIAL_SERVICE_LABELS[key]}”功能已在 Mode 下线，该官方平台地址不能访问。反馈请访问 ${MODE_ISSUES_URL}`,
    );
  }
  if (key) {
    throw new Error(
      `官方平台服务已下线，${key} 功能不再可用；反馈请访问 ${MODE_ISSUES_URL}`,
    );
  }
  throw new Error(`该官方平台地址在 Mode 已下线或未登记，不能访问。反馈请访问 ${MODE_ISSUES_URL}`);
}
