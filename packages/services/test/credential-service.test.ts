import assert from "node:assert/strict";
import test from "node:test";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

// 数据根隔离必须发生在导入 credentialService 之前：MODE_DATA_BASE_DIR 在模块加载时读取。
process.env.MODE_DATA_BASE_DIR = await mkdtemp(join(tmpdir(), "mode-credential-list-"));
const { createCredentialService } = await import("../src/credential/credentialService.js");

test("credential list(prefix)：只按键名前缀枚举，不返回也不解密值", async () => {
  const credentials = createCredentialService();
  await credentials.save("lan:peer:aaa:token", "secret-token");
  await credentials.save("lan:peer:aaa:meta", '{"host":"10.0.0.2"}');
  await credentials.save("lan:access:server-id", "sid");

  const peerKeys = await credentials.list("lan:peer:");
  assert.deepEqual([...peerKeys].sort(), ["lan:peer:aaa:meta", "lan:peer:aaa:token"]);

  assert.deepEqual(await credentials.list("nothing:"), []);

  // 删除后不再出现（配对删除依赖该语义）。
  await credentials.delete("lan:peer:aaa:meta");
  assert.deepEqual(await credentials.list("lan:peer:"), ["lan:peer:aaa:token"]);
});
