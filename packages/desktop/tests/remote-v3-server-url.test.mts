import assert from "node:assert/strict";
import test from "node:test";

import { normalizeRemoteServerUrl } from "../src/host/remoteV3/api.js";

test("v3 服务地址保留网关/反代路径前缀", () => {
  assert.equal(
    normalizeRemoteServerUrl("https://nas.example/app/mode/"),
    "https://nas.example/app/mode",
  );
  assert.equal(normalizeRemoteServerUrl("https://remote.example.com/"), "https://remote.example.com");
  assert.throws(() => normalizeRemoteServerUrl("http://nas.example/app/mode"), /HTTPS/u);
});
