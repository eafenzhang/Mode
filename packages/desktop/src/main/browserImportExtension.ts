// P1/T4：把扩展经 Native Messaging / 命名管道送来的 cookie 写入内置浏览器 session。
//
// 规格：`docs/specs/chrome-browser-import-extension.md` §3、§4.3、§5。
// 与现有 app-bound helper 路径互斥：调用方在 `browserImportExtension` 为真时不再读 Chrome DB。
import type { ChromeBrowserDataImportError } from "@mode/shared";

import {
  createBrowserImportChannel,
  type BrowserImportChannelLogger,
  type BrowserImportCookie,
} from "./browserImportChannel.js";

/** Electron `CookiesSetDetails` 的最小结构（测试可注入假 session）。 */
export interface BrowserImportCookieDetails {
  url: string;
  name: string;
  value: string;
  domain?: string;
  path?: string;
  secure?: boolean;
  httpOnly?: boolean;
  sameSite?: "unspecified" | "no_restriction" | "lax" | "strict";
  expirationDate?: number;
}

export interface BrowserImportCookieWriter {
  cookies: {
    set(details: BrowserImportCookieDetails): Promise<void>;
    flushStore: () => Promise<void>;
  };
}

export interface BrowserImportExtensionOptions {
  targetSession: BrowserImportCookieWriter;
  logger: BrowserImportChannelLogger;
  /** 等待用户点击扩展的上限；默认 65s（通道自身 TTL 60s）。 */
  timeoutMs?: number;
  channelPath?: string;
}

export interface BrowserImportExtensionResult {
  imported: number;
  skipped: number;
  failed: number;
  error?: ChromeBrowserDataImportError;
}

const WRITE_CONCURRENCY = 16;
const DEFAULT_TIMEOUT_MS = 65_000;

/**
 * 打开一次导入通道并等待扩展送数据。成功返回写入条数；超时/失败返回稳定错误。
 */
export async function importCookiesFromExtension(
  options: BrowserImportExtensionOptions,
): Promise<BrowserImportExtensionResult> {
  let resolveDone: ((count: number) => void) | undefined;
  const done = new Promise<number>((resolve) => {
    resolveDone = resolve;
  });

  const channel = await createBrowserImportChannel({
    channelPath: options.channelPath,
    logger: options.logger,
    onCookies: async (cookies) => {
      const stats = await writeCookies(options.targetSession, cookies, options.logger);
      resolveDone?.(stats.imported);
      return stats.imported;
    },
  });

  try {
    const imported = await Promise.race([
      done,
      delay(options.timeoutMs ?? DEFAULT_TIMEOUT_MS).then<number>(() => {
        throw new Error("channel_timeout");
      }),
    ]);
    options.logger.info("[browser-import] extension import finished", { imported });
    return { imported, skipped: 0, failed: 0 };
  } catch (error) {
    options.logger.warn("[browser-import] extension import failed", {
      reason: error instanceof Error ? error.message : String(error),
    });
    return {
      imported: 0,
      skipped: 0,
      failed: 0,
      error: "chrome_browser_data_import_unavailable",
    };
  } finally {
    await channel.close();
  }
}

async function writeCookies(
  targetSession: BrowserImportCookieWriter,
  cookies: BrowserImportCookie[],
  logger: BrowserImportChannelLogger,
): Promise<{ imported: number; skipped: number; failed: number }> {
  const details: BrowserImportCookieDetails[] = [];
  let skipped = 0;
  for (const cookie of cookies) {
    const mapped = toElectronCookie(cookie);
    if (mapped) details.push(mapped);
    else skipped += 1;
  }

  let imported = 0;
  let failed = 0;
  for (let offset = 0; offset < details.length; offset += WRITE_CONCURRENCY) {
    const batch = details.slice(offset, offset + WRITE_CONCURRENCY);
    const results = await Promise.allSettled(
      batch.map((detail) => targetSession.cookies.set(detail)),
    );
    for (const result of results) {
      if (result.status === "fulfilled") imported += 1;
      else failed += 1;
    }
  }
  await targetSession.cookies.flushStore();
  logger.info("[browser-import] extension cookies written", { imported, skipped, failed });
  return { imported, skipped, failed };
}

/** 扩展 cookie -> Electron `cookies.set` 参数；无法安全映射时返回 null。 */
export function toElectronCookie(cookie: BrowserImportCookie): BrowserImportCookieDetails | null {
  const name = stringField(cookie, "name");
  const value = stringField(cookie, "value");
  const domain = stringField(cookie, "domain");
  const path = stringField(cookie, "path") ?? "/";
  if (!name || value === null || !domain || !path.startsWith("/")) return null;
  const host = domain.replace(/^\./, "");
  if (!host) return null;
  const secure = cookie.secure === true;
  const sameSite = sameSiteValue(cookie.sameSite);
  const expirationDate =
    typeof cookie.expirationDate === "number" && Number.isFinite(cookie.expirationDate)
      ? cookie.expirationDate
      : undefined;
  return {
    url: `${secure ? "https" : "http"}://${host}${path}`,
    name,
    value,
    ...(domain.startsWith(".") ? { domain } : {}),
    path,
    secure,
    httpOnly: cookie.httpOnly === true,
    ...(sameSite ? { sameSite } : {}),
    ...(expirationDate !== undefined ? { expirationDate } : {}),
  };
}

function stringField(cookie: BrowserImportCookie, key: string): string | null {
  const value = cookie[key];
  return typeof value === "string" ? value : null;
}

function sameSiteValue(
  value: unknown,
): BrowserImportCookieDetails["sameSite"] | undefined {
  return value === "strict" || value === "lax" || value === "no_restriction" || value === "unspecified"
    ? value
    : undefined;
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => {
    const timer = setTimeout(resolve, ms);
    timer.unref?.();
  });
}
