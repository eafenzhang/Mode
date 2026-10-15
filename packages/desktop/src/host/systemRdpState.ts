/**
 * 被控端「系统 RDP 状态」探测（docs/specs/remote-desktop-system-rdp.md 接口节）。
 *
 * 只读 HKLM，**不需要提权**：
 * - `HKLM\SYSTEM\CurrentControlSet\Control\Terminal Server` → `fDenyTSConnections`
 *   （`0` = 系统远程桌面已启用）；
 * - `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion` → `EditionID`
 *   （家庭版不能承载 RDP 主机）。
 *
 * 读法用 `reg.exe query`（既有先例 desktopWindowsOpenFolderContextMenu.ts）。
 *
 * 两条纪律：
 * 1. **握手路径不做 IO**：server-info 的能力位取值函数是同步的，故探测结果带 TTL 缓存，
 *    `get()` 只读缓存；缓存过期时在后台补一次（不阻塞握手，也不引定时器）。
 * 2. **不谎报可用**：`enabled` 解析失败 → 整体返回 undefined（不发能力键 = 对端按「状态未知」
 *    呈现，仍允许发起）；瞬时失败保留上一次成功值（陈旧但真实，胜过空白）。
 *    版本号解析失败例外：`editionOk` 按 true 放行（只影响设备页置灰，误判会挡住能用的
 *    Pro 机器；真不支持时由 mstsc 自身报错），并记 warn。
 */
import { execFile } from "node:child_process";

/** 家庭版（及同族）EditionID：这些版本不提供 RDP 主机。 */
const FAMILY_EDITION_IDS = new Set([
  "Core",
  "CoreCountrySpecific",
  "CoreSingleLanguage",
  "CoreN",
  "CoreCountrySpecificN",
  "CoreSingleLanguageN",
]);

const TERMINAL_SERVER_KEY = "HKLM\\SYSTEM\\CurrentControlSet\\Control\\Terminal Server";
const CURRENT_VERSION_KEY = "HKLM\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion";

/** 默认缓存 TTL：5 分钟。注册表这两项只在用户手动改系统设置时变化。 */
export const SYSTEM_RDP_PROBE_TTL_MS = 5 * 60_000;

export interface SystemRdpState {
  /** 系统远程桌面已启用（`fDenyTSConnections === 0`）。 */
  enabled: boolean;
  /** 当前 Windows 版本支持承载 RDP 主机（非家庭版）。 */
  editionOk: boolean;
}

export interface SystemRdpProbeDeps {
  /** 执行 `reg.exe query <key> /v <name>` 并返回 stdout。注入以便测试。 */
  runRegQuery(key: string, valueName: string): Promise<string>;
  /** 日志：探测失败/版本号无法判定时留痕（warn 级）。 */
  warn?(message: string): void;
  now?(): number;
  ttlMs?: number;
}

export interface SystemRdpProbe {
  /** 同步读缓存；未就绪或从未成功探测过时为 undefined。过期时顺带在后台补一次。 */
  get(): SystemRdpState | undefined;
  /** 强制探测一次（设置页刷新用）。返回最新值；失败且无历史值时为 undefined。 */
  refresh(): Promise<SystemRdpState | undefined>;
}

/**
 * 从 `reg.exe query` 输出里取指定值。输出形如：
 * `    fDenyTSConnections    REG_DWORD    0x0`
 * 值名与类型串不随系统语言变化，故按行匹配。
 */
export function parseRegValue(stdout: string, valueName: string): string | undefined {
  const pattern = new RegExp(`^\\s*${valueName}\\s+REG_\\w+\\s+(.*?)\\s*$`, "i");
  for (const line of stdout.split(/\r?\n/u)) {
    const matched = pattern.exec(line);
    if (matched?.[1]) {
      return matched[1];
    }
  }
  return undefined;
}

/** `fDenyTSConnections` → 是否已启用（`0` = 启用；支持 `0x0` 与十进制）。 */
export function parseFDenyTSConnections(raw: string | undefined): boolean | undefined {
  if (raw === undefined) {
    return undefined;
  }
  const normalized = raw.trim().toLowerCase();
  const value = normalized.startsWith("0x")
    ? Number.parseInt(normalized.slice(2), 16)
    : Number.parseInt(normalized, 10);
  if (!Number.isFinite(value)) {
    return undefined;
  }
  return value === 0;
}

/**
 * EditionID → 是否能承载 RDP 主机。
 * 无法判定（空/未知）时按 true 放行（见文件头纪律 2），由调用方记 warn。
 */
export function isRdpHostEditionSupported(editionId: string | undefined): {
  editionOk: boolean;
  recognized: boolean;
} {
  const trimmed = editionId?.trim();
  if (!trimmed) {
    return { editionOk: true, recognized: false };
  }
  return { editionOk: !FAMILY_EDITION_IDS.has(trimmed), recognized: true };
}

/** 生产实现：`reg.exe query`。失败（非 0 退出码/找不到值）抛错，由探测层降级。 */
export async function runRegQuery(key: string, valueName: string): Promise<string> {
  return await new Promise<string>((resolve, reject) => {
    execFile(
      "reg.exe",
      ["query", key, "/v", valueName],
      { windowsHide: true, timeout: 5_000 },
      (error, stdout) => {
        if (error) {
          reject(error);
          return;
        }
        resolve(stdout);
      },
    );
  });
}

export function createSystemRdpProbe(deps: SystemRdpProbeDeps): SystemRdpProbe {
  const now = deps.now ?? Date.now;
  const ttlMs = deps.ttlMs ?? SYSTEM_RDP_PROBE_TTL_MS;
  /** 上一次成功探测的值 + 时刻；`at` 为 null 表示从未成功过。 */
  let cached: { value: SystemRdpState; at: number } | null = null;
  let inFlight: Promise<SystemRdpState | undefined> | null = null;

  const isFresh = (): boolean => cached !== null && now() - cached.at < ttlMs;

  const probe = async (): Promise<SystemRdpState | undefined> => {
    try {
      const [tsOutput, editionOutput] = await Promise.all([
        deps.runRegQuery(TERMINAL_SERVER_KEY, "fDenyTSConnections"),
        deps.runRegQuery(CURRENT_VERSION_KEY, "EditionID").catch(() => ""),
      ]);
      const enabled = parseFDenyTSConnections(
        parseRegValue(tsOutput, "fDenyTSConnections"),
      );
      if (enabled === undefined) {
        // 读到了 key 但取不到值：宁可不发能力键，也不猜。
        deps.warn?.("[system-rdp] fDenyTSConnections 解析失败，按状态未知处理");
        return cached?.value;
      }
      const edition = isRdpHostEditionSupported(
        parseRegValue(editionOutput, "EditionID"),
      );
      if (!edition.recognized) {
        deps.warn?.("[system-rdp] 无法判定 Windows 版本，按支持处理（由 mstsc 侧兜底）");
      }
      const value: SystemRdpState = { enabled, editionOk: edition.editionOk };
      cached = { value, at: now() };
      return value;
    } catch (error) {
      // 瞬时失败保留上一次成功值（陈旧但真实）；从未成功过则维持未知。
      deps.warn?.(
        `[system-rdp] 注册表探测失败，保留上次结果: ${
          error instanceof Error ? error.message : String(error)
        }`,
      );
      return cached?.value;
    }
  };

  return {
    get(): SystemRdpState | undefined {
      if (isFresh()) {
        return cached?.value;
      }
      // 后台补一次；不 await（握手路径不做 IO）。并发握手共用同一个 in-flight 探测。
      if (!inFlight) {
        inFlight = probe().finally(() => {
          inFlight = null;
        });
      }
      return cached?.value;
    },
    async refresh(): Promise<SystemRdpState | undefined> {
      if (!inFlight) {
        inFlight = probe().finally(() => {
          inFlight = null;
        });
      }
      return await inFlight;
    },
  };
}
