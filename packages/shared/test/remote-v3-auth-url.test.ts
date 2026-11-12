import assert from "node:assert/strict";
import test from "node:test";

import { buildGithubDeviceAuthorizationUrl } from "../src/remoteV3.js";

test("GitHub Device 授权地址预填 user_code", () => {
  assert.equal(
    buildGithubDeviceAuthorizationUrl("https://github.com/login/device", "CABE-3B5E"),
    "https://github.com/login/device?user_code=CABE-3B5E",
  );
});

test("保留已有查询参数且不重复写 user_code", () => {
  assert.equal(
    buildGithubDeviceAuthorizationUrl(
      "https://github.com/login/device?user_code=AAAA-1111&locale=zh",
      "BBBB-2222",
    ),
    "https://github.com/login/device?user_code=AAAA-1111&locale=zh",
  );
});

test("非法地址原样返回，登录流程不因构造失败中断", () => {
  assert.equal(buildGithubDeviceAuthorizationUrl("not a url", "CABE-3B5E"), "not a url");
});
