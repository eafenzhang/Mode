/**
 * 网页门户静态页（docs/specs/cloud-personal-accounts.md §手机/浏览器门户）。
 *
 * 页面自包含（无外部字体/CDN），只通过同源 `/v1/*` 接口工作；
 * 会话走 HttpOnly Cookie，由 controlAuthPassword/controlAuthGithub 下发。
 */
import { readFileSync } from "node:fs";
import type { ServerResponse } from "node:http";

const PORTAL_HTML = readFileSync(new URL("./portal.html", import.meta.url), "utf8");
const PORTAL_CSP =
  "default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; connect-src 'self'; img-src 'self' data:; base-uri 'none'; form-action 'self'; frame-ancestors 'none'";

/** 门户路径：`/`、`/portal`、`/portal.html`（`/d/*` 仍走隧道转发）。 */
export function isPortalPath(pathname: string): boolean {
  return pathname === "/" || pathname === "/portal" || pathname === "/portal.html";
}

export function servePortal(res: ServerResponse): void {
  res.writeHead(200, {
    "content-type": "text/html; charset=utf-8",
    "cache-control": "no-store",
    "content-security-policy": PORTAL_CSP,
    "x-content-type-options": "nosniff",
    "referrer-policy": "no-referrer",
  });
  res.end(PORTAL_HTML);
}
