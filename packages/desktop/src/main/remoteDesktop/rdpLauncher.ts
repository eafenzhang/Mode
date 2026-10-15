/**
 * 主控端系统 RDP 启动器（docs/specs/remote-desktop-system-rdp.md 接口节）。
 *
 * 职责：校验 → 生成临时 `.rdp` → `spawn mstsc.exe <file>` → 跟踪进程生命周期 → 清理临时文件。
 * 画面/输入/剪贴板/音频全部由 Windows RDP 承担，本模块**不参与会话内容**。
 *
 * 两条安全纪律（spec「安全」节，不可妥协）：
 * 1. **`.rdp` 是 `key:type:value` 文本格式**，值里含换行即可注入任意选项（如 `alternate shell`）。
 *    `address` 来自发现结果（对端可控）、`username` 来自用户输入，两者都必须整串校验；
 *    失败即拒绝生成文件（不截断、不猜测）。
 * 2. 模板不含 `password 51:b:`，且固定 `enablecredsspsupport:i:1` / `authentication level:i:2`：
 *    密码交给 Windows 登录框，认证等级只升不降。
 */
import { randomUUID } from "node:crypto";
import { mkdir, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import {
  DEFAULT_RDP_PORT,
  type RdpConnectionOptions,
  type RdpConnectResult,
  type RdpConnectFailureReason,
  type RdpSessionRecord,
} from "@mode/shared/remote-desktop-rdp";

/** mstsc 可执行名（Windows 自带，随系统 PATH）。 */
export const MSTSC_EXECUTABLE = "mstsc.exe";

/** 生成阶段能拦下的失败原因（`launch_failed` 属于启动阶段，见 RdpConnectFailureReason）。 */
export type RdpBuildFailureReason = Exclude<RdpConnectFailureReason, "launch_failed">;

export interface RdpBuildInput {
  address: string;
  options: RdpConnectionOptions;
}

const IPV4_PATTERN = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/u;
const HOSTNAME_PATTERN = /^[A-Za-z0-9](?:[A-Za-z0-9_-]{0,62})(?:\.[A-Za-z0-9](?:[A-Za-z0-9_-]{0,62}))*\.?$/u;
const IPV6_PATTERN = /^[0-9A-Fa-f:.]+$/u;

/**
 * 地址校验：IPv4 / IPv6 / 主机名。
 * 拒绝一切控制字符与空白——`full address` 是 `.rdp` 行，换行即选项注入。
 */
export function isValidRdpAddress(raw: string): boolean {
  const value = raw.trim();
  if (value.length === 0 || value.length > 253) {
    return false;
  }
  const ipv4 = IPV4_PATTERN.exec(value);
  if (ipv4) {
    return ipv4.slice(1).every((octet) => Number(octet) <= 255);
  }
  if (value.includes(":")) {
    // IPv6：只允许十六进制数字、冒号与点（覆盖内嵌 IPv4 写法）。
    return IPV6_PATTERN.test(value) && value.length >= 2;
  }
  return HOSTNAME_PATTERN.test(value);
}

/**
 * 用户名校验：非空、无控制字符、无换行。
 * 允许 `DOMAIN\user` 与 `user@domain`（RDP 接受两种域写法）。
 */
export function isValidRdpUsername(raw: string): boolean {
  const value = raw.trim();
  if (value.length === 0 || value.length > 256) {
    return false;
  }
  // eslint-disable-next-line no-control-regex -- 本校验的目的正是拦截控制字符。
  return !/[\u0000-\u001f\u007f]/u.test(value);
}

function formatAddress(address: string, port: number | undefined): string {
  const trimmed = address.trim();
  // IPv6 含冒号：加端口会与地址语法混淆，一律用默认端口（不追加端口后缀）。
  if (trimmed.includes(":") || port === undefined || port === DEFAULT_RDP_PORT) {
    return trimmed;
  }
  return `${trimmed}:${port}`;
}

/**
 * 生成 `.rdp` 内容（纯函数）。
 * 返回判别联合而非抛错：调用方必须显式处理「拒绝生成」，避免静默降级成截断后的值。
 */
export function buildRdpFile(
  input: RdpBuildInput,
): { ok: true; content: string } | { ok: false; reason: RdpBuildFailureReason } {
  if (!isValidRdpAddress(input.address)) {
    return { ok: false, reason: "invalid_address" };
  }
  const rawUsername = input.options.username?.trim() ?? "";
  if (rawUsername.length > 0 && !isValidRdpUsername(rawUsername)) {
    return { ok: false, reason: "invalid_username" };
  }
  const { fullscreen, multimon, clipboard, audio, port } = input.options;
  const audioMode = audio === "remote" ? 1 : audio === "disabled" ? 2 : 0;
  const lines = [
    `full address:s:${formatAddress(input.address, port)}`,
    // 只预填用户名；密码由 Windows 登录框收集（不写 password 51:b:）。
    ...(rawUsername.length > 0 ? [`username:s:${rawUsername}`] : []),
    // 窗口/全屏：缺省窗口（用户的会话不该抢走整个屏幕）。
    `screen mode id:i:${fullscreen === true ? 2 : 1}`,
    `use multimon:i:${multimon === true ? 1 : 0}`,
    "smart sizing:i:1",
    "dynamic resolution:i:1",
    // 剪贴板缺省开（远程桌面最常被期待的能力）。
    `redirectclipboard:i:${clipboard === false ? 0 : 1}`,
    `audiomode:i:${audioMode}`,
    "redirectprinters:i:0",
    "drivestoredirect:s:",
    // 认证：只升不降（不写 password、不关 NLA）。
    "authentication level:i:2",
    "enablecredsspsupport:i:1",
    "prompt for credentials:i:1",
    "promptcredentialonce:i:1",
    // 带宽与画质交给 RDP 自身的自适应。
    "networkautodetect:i:1",
    "bandwidthautodetect:i:1",
    "connection type:i:7",
    "bitmapcachepersistenable:i:1",
    "keyboardhook:i:2",
    "compression:i:1",
  ];
  return { ok: true, content: `${lines.join("\r\n")}\r\n` };
}

/** 应用内可见的一条 RDP 会话：形状真源在 `@mode/shared/remote-desktop-rdp`。 */

/** 最小子进程结构：只为可注入测试，不绑定 child_process 的具体类型。 */
export interface RdpChildLike {
  pid?: number | undefined;
  on(event: "exit", listener: (code: number | null) => void): unknown;
  on(event: "spawn", listener: () => void): unknown;
  on(event: "error", listener: (error: Error) => void): unknown;
}

export type RdpSpawner = (command: string, args: readonly string[]) => RdpChildLike;

export interface RdpLauncherLogger {
  info(message: string, detail?: unknown): void;
  warn(message: string, detail?: unknown): void;
}

export interface RdpLauncherDeps {
  spawn: RdpSpawner;
  /** 临时 `.rdp` 目录（生产：`<temp>/mode-rdp`）。 */
  tempDir: string;
  /** 终止进程树（生产：`taskkill /PID <pid> /T /F`）。缺省仅尝试空操作。 */
  killTree?(pid: number): Promise<void>;
  logger: RdpLauncherLogger;
  now?(): number;
  randomId?(): string;
}

export interface RdpConnectInput {
  serverId: string;
  name: string;
  address: string;
  options: RdpConnectionOptions;
}

export interface RdpLauncher {
  connect(input: RdpConnectInput): Promise<RdpConnectResult>;
  list(): RdpSessionRecord[];
  disconnect(sessionId: string): Promise<{ ok: boolean }>;
  /** 应用退出：删除全部临时 `.rdp` 并清空跟踪；**不终止会话**（mstsc 是独立进程）。 */
  cleanupOnQuit(): Promise<void>;
}

export function createRdpLauncher(deps: RdpLauncherDeps): RdpLauncher {
  const now = deps.now ?? Date.now;
  const randomId = deps.randomId ?? randomUUID;
  const sessions = new Map<string, { record: RdpSessionRecord; rdpFilePath: string }>();
  let dirReady: Promise<void> | null = null;

  const ensureDir = async (): Promise<void> => {
    dirReady ??= mkdir(deps.tempDir, { recursive: true }).then(() => undefined);
    return await dirReady;
  };

  const removeFile = async (rdpFilePath: string): Promise<void> => {
    try {
      await rm(rdpFilePath, { force: true });
    } catch (error) {
      deps.logger.warn("[rdp] 临时 .rdp 删除失败", error);
    }
  };

  const forget = async (sessionId: string): Promise<void> => {
    const entry = sessions.get(sessionId);
    if (!entry) {
      return;
    }
    sessions.delete(sessionId);
    await removeFile(entry.rdpFilePath);
  };

  return {
    async connect(input: RdpConnectInput): Promise<RdpConnectResult> {
      const built = buildRdpFile({ address: input.address, options: input.options });
      if (!built.ok) {
        deps.logger.warn(`[rdp] 连接参数被拒（${built.reason}）`, { serverId: input.serverId });
        return { ok: false, reason: built.reason };
      }
      const sessionId = randomId();
      const rdpFilePath = join(deps.tempDir, `${sessionId}.rdp`);
      try {
        await ensureDir();
        await writeFile(rdpFilePath, built.content, "utf8");
      } catch (error) {
        deps.logger.warn("[rdp] 写入临时 .rdp 失败", error);
        return { ok: false, reason: "launch_failed" };
      }

      const spawned = await new Promise<RdpConnectResult>((resolve) => {
        let settled = false;
        const settle = (result: RdpConnectResult): void => {
          if (!settled) {
            settled = true;
            resolve(result);
          }
        };
        let child: RdpChildLike;
        try {
          child = deps.spawn(MSTSC_EXECUTABLE, [rdpFilePath]);
        } catch (error) {
          deps.logger.warn("[rdp] 启动 mstsc 失败", error);
          settle({ ok: false, reason: "launch_failed" });
          return;
        }
        child.on("error", (error: Error) => {
          deps.logger.warn("[rdp] mstsc 启动错误", error);
          settle({ ok: false, reason: "launch_failed" });
        });
        child.on("spawn", () => {
          const pid = child.pid;
          if (pid === undefined) {
            settle({ ok: false, reason: "launch_failed" });
            return;
          }
          const record: RdpSessionRecord = {
            sessionId,
            serverId: input.serverId,
            name: input.name,
            address: input.address,
            pid,
            startedAt: now(),
          };
          sessions.set(sessionId, { record, rdpFilePath });
          // 会话结束（用户关窗 / 断开 / 进程崩溃）即移除跟踪并删临时文件。
          child.on("exit", () => {
            deps.logger.info("[rdp] 会话结束", { serverId: input.serverId, pid });
            void forget(sessionId);
          });
          deps.logger.info("[rdp] 会话已启动", {
            serverId: input.serverId,
            name: input.name,
            address: input.address,
            pid,
          });
          settle({ ok: true, session: record });
        });
      });

      if (!spawned.ok) {
        // 启动失败：不留半截临时文件。
        await removeFile(rdpFilePath);
      }
      return spawned;
    },

    list(): RdpSessionRecord[] {
      return [...sessions.values()].map((entry) => entry.record);
    },

    async disconnect(sessionId: string): Promise<{ ok: boolean }> {
      const entry = sessions.get(sessionId);
      if (!entry) {
        return { ok: false };
      }
      if (deps.killTree) {
        try {
          await deps.killTree(entry.record.pid);
        } catch (error) {
          deps.logger.warn("[rdp] 终止会话进程失败", error);
        }
      }
      // 不等 exit 事件：进程可能已经消失（那时事件不会再触发）。
      await forget(sessionId);
      return { ok: true };
    },

    async cleanupOnQuit(): Promise<void> {
      const files = [...sessions.values()].map((entry) => entry.rdpFilePath);
      sessions.clear();
      await Promise.all(files.map(removeFile));
    },
  };
}
