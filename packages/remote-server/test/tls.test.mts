import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { request } from "node:https";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { startRemoteServer } from "../src/index.js";

function hasOpenssl(): boolean {
  try {
    execFileSync("openssl", ["version"], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
}

function get(url: string): Promise<{ status: number; body: string }> {
  return new Promise((resolve, reject) => {
    const req = request(url, { rejectUnauthorized: false }, (res) => {
      let body = "";
      res.setEncoding("utf8");
      res.on("data", (chunk) => {
        body += chunk;
      });
      res.on("end", () => resolve({ status: res.statusCode ?? 0, body }));
    });
    req.once("error", reject);
    req.end();
  });
}

test(
  "remote-server 可直接终止 TLS：URL/Relay 使用 https/wss",
  { skip: hasOpenssl() ? false : "openssl 不可用" },
  async () => {
    const dir = await mkdtemp(join(tmpdir(), "mode-remote-tls-"));
    const keyFile = join(dir, "key.pem");
    const certFile = join(dir, "cert.pem");
    try {
      execFileSync("openssl", [
        "req",
        "-x509",
        "-newkey",
        "rsa:2048",
        "-nodes",
        "-keyout",
        keyFile,
        "-out",
        certFile,
        "-days",
        "1",
        "-subj",
        "/CN=127.0.0.1",
        "-addext",
        "subjectAltName=IP:127.0.0.1",
      ]);
      const server = await startRemoteServer({
        port: 0,
        host: "127.0.0.1",
        dbPath: ":memory:",
        secret: "tls-test-secret-long-enough",
        tls: {
          key: await readFile(keyFile),
          cert: await readFile(certFile),
        },
        githubVerifier: async () => ({ providerUserId: "tls", login: "tls" }),
      });
      try {
        assert.match(server.url, /^https:\/\/127\.0\.0\.1:\d+$/);
        assert.match(server.relayEndpoint, /^wss:\/\/127\.0\.0\.1:\d+\/ws\/v1\/relay$/);
        const response = await get(`${server.url}/health`);
        assert.equal(response.status, 200);
        assert.equal(response.body, "ok");
      } finally {
        await server.close();
      }
    } finally {
      await rm(dir, { recursive: true, force: true });
    }
  },
);
