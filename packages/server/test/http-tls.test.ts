import assert from "node:assert/strict";
import test from "node:test";
import { request as httpsRequest } from "node:https";
import type { AddressInfo } from "node:net";
import forge from "node-forge";
import type { ServiceCollection } from "@mode/services";

import { createHttpServer } from "../src/http.js";

// LAN 监听 TLS（docs/specs/lan-remote-desktop.md 安全边界）：提供 key/cert 时进程
// 直接终止 HTTPS（证书须被客户端信任；测试用自签名证书 + 关闭校验仅用于本机连通用例）。
function createSelfSignedCertificate(): { key: string; cert: string } {
  const keys = forge.pki.rsa.generateKeyPair(2048);
  const cert = forge.pki.createCertificate();
  cert.publicKey = keys.publicKey;
  cert.serialNumber = "01";
  cert.validity.notBefore = new Date(Date.now() - 60_000);
  cert.validity.notAfter = new Date(Date.now() + 86_400_000);
  const attributes = [{ name: "commonName", value: "127.0.0.1" }];
  cert.setSubject(attributes);
  cert.setIssuer(attributes);
  cert.setExtensions([
    { name: "subjectAltName", altNames: [{ type: 7, ip: "127.0.0.1" }] },
  ]);
  cert.sign(keys.privateKey, forge.md.sha256.create());
  return {
    key: forge.pki.privateKeyToPem(keys.privateKey),
    cert: forge.pki.certificateToPem(cert),
  };
}

function getInsecure(url: string): Promise<{ status: number; body: string }> {
  return new Promise((resolve, reject) => {
    const req = httpsRequest(
      url,
      { rejectUnauthorized: false },
      (res) => {
        let body = "";
        res.setEncoding("utf8");
        res.on("data", (chunk) => {
          body += chunk;
        });
        res.on("end", () => resolve({ status: res.statusCode ?? 0, body }));
      },
    );
    req.on("error", reject);
    req.end();
  });
}

test("提供 tls 时 createHttpServer 直接终止 HTTPS", async () => {
  const tls = createSelfSignedCertificate();
  const server = createHttpServer({} as unknown as ServiceCollection, 0, {
    host: "127.0.0.1",
    tls,
  });
  await new Promise<void>((resolve) => server.once("listening", () => resolve()));
  const { port } = server.address() as AddressInfo;
  try {
    const response = await getInsecure(`https://127.0.0.1:${port}/api/server-info`);
    assert.equal(response.status, 200, response.body);
    assert.match(response.body, /"serverId"/u, response.body);
  } finally {
    await new Promise<void>((resolve) => server.close(() => resolve()));
  }
});
