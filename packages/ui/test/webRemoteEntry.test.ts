import assert from "node:assert/strict";
import test from "node:test";

import { resolveWebRemoteAddress, resolveWebRemoteUrl } from "../src/lib/webRemoteEntry.js";

const base = {
  enabled: true,
  port: 45880,
  addresses: [],
  pairCode: null,
  clients: [],
};

test("Web 远控地址优先使用 Host 回报的第一个监听地址", () => {
  const state = { ...base, addresses: ["192.168.1.4:45880", "100.90.96.50:45880"] };
  assert.equal(resolveWebRemoteAddress(state), "192.168.1.4:45880");
  assert.equal(resolveWebRemoteUrl(state), "http://192.168.1.4:45880/");
});

test("Web 远控地址在地址列表为空时回退本机端口", () => {
  assert.equal(resolveWebRemoteAddress(base), "127.0.0.1:45880");
  assert.equal(resolveWebRemoteUrl(base), "http://127.0.0.1:45880/");
});

test("Web 远控地址在未启用/无端口时为空，不拼出假地址", () => {
  assert.equal(resolveWebRemoteAddress(null), "");
  assert.equal(resolveWebRemoteUrl(null), "");
  assert.equal(resolveWebRemoteAddress({ ...base, enabled: false, port: null }), "");
});
