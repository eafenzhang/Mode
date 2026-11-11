import assert from "node:assert/strict";
import test from "node:test";

import { createEncryptedCredentialStore, type WebCredentialStore } from "../src/web/index.js";

/** 模拟 IndexedDB：字符串与结构化对象（CryptoKey）分开存。 */
function createBackingStore(): WebCredentialStore & {
  strings: Map<string, string>;
  objects: Map<string, unknown>;
} {
  const strings = new Map<string, string>();
  const objects = new Map<string, unknown>();
  return {
    strings,
    objects,
    get: async (key) => strings.get(key) ?? null,
    set: async (key, value) => {
      strings.set(key, value);
    },
    delete: async (key) => {
      strings.delete(key);
      objects.delete(key);
    },
    getValue: async <T,>(key: string) => (objects.get(key) as T | undefined) ?? null,
    setValue: async <T,>(key: string, value: T) => {
      objects.set(key, value);
    },
  };
}

test("加密存储：写入为密文，读取还原明文，密钥可跨实例复用", async () => {
  const backing = createBackingStore();
  const store = createEncryptedCredentialStore(backing);

  await store.set("profile", JSON.stringify({ privateKey: "secret-key", accessToken: "t-1" }));
  const raw = backing.strings.get("profile")!;
  assert.ok(!raw.includes("secret-key"), "落盘内容不得包含私钥明文");
  assert.ok(!raw.includes("t-1"), "落盘内容不得包含 token 明文");
  assert.equal(JSON.parse(raw).v, 1);

  const restored = JSON.parse((await store.get("profile"))!) as { privateKey: string };
  assert.equal(restored.privateKey, "secret-key");

  // 新的 wrapper 复用同一 backing：密钥已持久化，仍能解密。
  const reopened = createEncryptedCredentialStore(backing);
  assert.equal(
    (JSON.parse((await reopened.get("profile"))!) as { accessToken: string }).accessToken,
    "t-1",
  );
});

test("旧明文值读取兼容，下一次写入升级为密文", async () => {
  const backing = createBackingStore();
  backing.strings.set("profile", JSON.stringify({ privateKey: "legacy" }));
  const store = createEncryptedCredentialStore(backing);
  assert.equal((JSON.parse((await store.get("profile"))!) as { privateKey: string }).privateKey, "legacy");

  await store.set("profile", JSON.stringify({ privateKey: "legacy-2" }));
  assert.ok(!backing.strings.get("profile")!.includes("legacy-2"));
});

test("不支持对象存储时原样返回 inner（localStorage 回落不假装加密）", async () => {
  const plain: WebCredentialStore = {
    get: async () => null,
    set: async () => undefined,
    delete: async () => undefined,
  };
  assert.equal(createEncryptedCredentialStore(plain), plain);
});

test("delete 透传，删除后读取为空", async () => {
  const backing = createBackingStore();
  const store = createEncryptedCredentialStore(backing);
  await store.set("profile", "value");
  assert.ok(await store.get("profile"));
  await store.delete("profile");
  assert.equal(await store.get("profile"), null);
});
