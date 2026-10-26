// mymode.cloud 静态文件服务（在 NAS 的 node:24-alpine 容器内运行）。
// 因为 NAS 拉取不了 Docker Hub 镜像，用现成的 node:24-alpine 替代 nginx:alpine。
// 站点文件只读挂载在 /srv/public，监听 80 端口（由 docker 映射到宿主 127.0.0.1:8095）。
"use strict";

const http = require("node:http");
const fs = require("node:fs");
const path = require("node:path");

const ROOT = "/srv/public";
const PORT = 80;

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

http
  .createServer((req, res) => {
    if (req.method !== "GET" && req.method !== "HEAD") {
      res.writeHead(405, { Allow: "GET, HEAD" }).end();
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
      res.writeHead(400).end();
      return;
    }
    if (pathname.endsWith("/")) pathname += "index.html";
    const file = path.normalize(path.join(ROOT, pathname));
    if (!file.startsWith(ROOT + path.sep) && file !== ROOT) {
      res.writeHead(403).end();
      return;
    }
    fs.stat(file, (err, st) => {
      const target = !err && st.isDirectory() ? path.join(file, "index.html") : file;
      fs.readFile(target, (readErr, buf) => {
        if (readErr) {
          res.writeHead(404, { "Content-Type": "text/plain; charset=utf-8" }).end("404 Not Found");
          return;
        }
        const ext = path.extname(target).toLowerCase();
        res.writeHead(200, {
          "Content-Type": MIME[ext] || "application/octet-stream",
          // HTML 不缓存以便更新即时生效；其余资源短缓存
          "Cache-Control": ext === ".html" ? "no-cache" : "public, max-age=600",
        });
        res.end(req.method === "HEAD" ? undefined : buf);
      });
    });
  })
  .listen(PORT, () => console.log(`mymode-site static server listening on :${PORT}`));
