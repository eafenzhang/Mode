import assert from "node:assert/strict";
import test from "node:test";
import type { AddressInfo } from "node:net";
import type { ServiceCollection } from "@mode/services";
import { serverRemoteInfoSchema } from "@mode/shared";
import { createHttpServer } from "../src/http.js";

async function fetchServerInfo(options: { computerUseCapability?: boolean }) {
  const server = createHttpServer({} as unknown as ServiceCollection, 0, {
    host: "127.0.0.1",
    ...(options.computerUseCapability !== undefined
      ? { computerUseCapability: options.computerUseCapability }
      : {}),
  });
  await new Promise<void>((resolve) => server.once("listening", () => resolve()));
  const { port } = server.address() as AddressInfo;
  try {
    const response = await fetch(`http://127.0.0.1:${port}/api/server-info`);
    assert.equal(response.status, 200);
    const parsed = serverRemoteInfoSchema.safeParse(await response.json());
    assert.equal(parsed.success, true, "server-info 必须通过共享 schema");
    return parsed.success ? parsed.data : undefined;
  } finally {
    await new Promise<void>((resolve) => server.close(() => resolve()));
  }
}

test("声明 computerUseCapability:true 时 capabilities.computerUse 为 true", async () => {
  const info = await fetchServerInfo({ computerUseCapability: true });
  assert.equal(info?.capabilities.computerUse, true);
});

test("声明 computerUseCapability:false 时键存在且为 false", async () => {
  const info = await fetchServerInfo({ computerUseCapability: false });
  assert.equal(info?.capabilities.computerUse, false);
});

test("未声明时键缺省（其余消费方与旧对端行为不变）", async () => {
  const info = await fetchServerInfo({});
  assert.equal(info?.capabilities.computerUse, undefined);
});
