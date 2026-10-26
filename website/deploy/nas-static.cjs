// mymode.cloud 静态文件服务（在 NAS 的 node:24-alpine 容器内运行）。
// 因为 NAS 拉取不了 Docker Hub 镜像，用现成的 node:24-alpine 替代 nginx:alpine。
// 站点文件挂载在 /srv/public（容器内需可写：版本号刷新进程要更新 version.json），
// 监听 80 端口（由 docker 映射到宿主 127.0.0.1:8095）。
"use strict";

const http = require("node:http");
const fs = require("node:fs/promises");
const path = require("node:path");

const ROOT = process.env.SITE_ROOT || "/srv/public";
const PORT = Number(process.env.SITE_PORT) || 80;

// GitHub API 在国内直连不稳定，由 NAS 侧定时刷新版本号写入站点目录，
// 页面同源读取 /version.json（release 每天最多几次，10 分钟一轮远低于限额）。
const RELEASE_API = "https://api.github.com/repos/eafenzhang/Mode/releases/latest";
const VERSION_FILE = path.join(ROOT, "version.json");
const TAG_RE = /^v\d+\.\d+\.\d+$/;

async function refreshVersion() {
  try {
    const resp = await fetch(RELEASE_API, {
      headers: { "User-Agent": "mymode-site" },
      signal: AbortSignal.timeout(15000),
    });
    if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
    const d = await resp.json();
    if (typeof d.tag_name !== "string" || !TAG_RE.test(d.tag_name)) {
      throw new Error(`unexpected tag_name: ${d.tag_name}`);
    }
    const body = JSON.stringify({ tag: d.tag_name }) + "\n";
    const current = await fs.readFile(VERSION_FILE, "utf8").catch(() => null);
    if (current === body) return;
    const tmp = VERSION_FILE + ".tmp";
    await fs.writeFile(tmp, body);
    await fs.rename(tmp, VERSION_FILE);
    console.log("version.json updated:", d.tag_name);
  } catch (err) {
    console.log("version refresh failed:", err && err.message ? err.message : String(err));
  }
}

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".png": "image/png",
  ".jpg": "image/jpeg",
  ".webp": "image/webp",
  ".svg": "image/svg+xml",
  ".ico": "image/x-icon",
  ".txt": "text/plain; charset=utf-8",
  ".woff2": "font/woff2",
};

// 错误响应统一带 no-store：CF 会按扩展名对可缓存对象做边缘缓存，
// 404 一旦被缓存会在 TTL 内遮住后续部署的同名文件（已实际踩过）。
function fail(res, code, msg, extra) {
  res.writeHead(code, {
    "Content-Type": "text/plain; charset=utf-8",
    "Cache-Control": "no-store",
    ...extra,
  });
  res.end(msg);
}

http
  .createServer(async (req, res) => {
    if (req.method !== "GET" && req.method !== "HEAD") {
      fail(res, 405, "Method Not Allowed", { Allow: "GET, HEAD" });
      return;
    }
    if (req.url === "/healthz") {
      res.writeHead(200, { "Content-Type": "text/plain" }).end("ok");
      return;
    }
    let pathname;
    try {
      pathname = decodeURIComponent(new URL(req.url, "http://localhost").pathname);
    } catch {
      fail(res, 400, "Bad Request");
      return;
    }
    if (pathname.endsWith("/")) pathname += "index.html";
    const file = path.normalize(path.join(ROOT, pathname));
    const rel = path.relative(ROOT, file);
    if (rel.startsWith("..") || path.isAbsolute(rel)) {
      fail(res, 403, "Forbidden");
      return;
    }
    try {
      const st = await fs.stat(file).catch(() => null);
      const target = st && st.isDirectory() ? path.join(file, "index.html") : file;
      const buf = await fs.readFile(target);
      const ext = path.extname(target).toLowerCase();
      // HTML/CSS/JS 改动需即时生效（体积小，重验代价可忽略），不缓存；
      // 图片等资源短缓存
      const noCache = [".html", ".css", ".js", ".json", ".txt", ".svg"].includes(ext);
      res.writeHead(200, {
        "Content-Type": MIME[ext] || "application/octet-stream",
        "Cache-Control": noCache ? "no-cache" : "public, max-age=600",
      });
      res.end(req.method === "HEAD" ? undefined : buf);
    } catch {
      fail(res, 404, "404 Not Found");
    }
  })
  .listen(PORT, () => console.log(`mymode-site static server listening on :${PORT}`));

refreshVersion();
setInterval(refreshVersion, 10 * 60 * 1000);
