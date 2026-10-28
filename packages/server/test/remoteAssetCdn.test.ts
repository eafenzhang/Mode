import assert from "node:assert/strict";
import test from "node:test";

import {
  assertRemoteCdnBaseVersionMatches,
  buildReleaseAssetUrlCandidates,
  buildReleaseBaseCandidates,
} from "../src/remote/remoteAssetCdn.js";

const RELEASE_ROOT = "https://github.com/eafenzhang/Mode/releases/download";

test("GitHub Release 的 v 前缀版本目录不重复拼版本", () => {
  const versioned = `${RELEASE_ROOT}/v0.0.94`;
  assert.deepEqual(buildReleaseBaseCandidates([versioned], "0.0.94"), [versioned]);
});

test("无 v 前缀的版本目录同样识别为固定版本", () => {
  const versioned = "https://cdn.example.com/remote/0.0.94";
  assert.deepEqual(buildReleaseBaseCandidates([versioned], "0.0.94"), [versioned]);
});

test("发布根目录仍生成版本候选和原根候选", () => {
  assert.deepEqual(buildReleaseBaseCandidates([RELEASE_ROOT], "0.0.94"), [
    `${RELEASE_ROOT}/0.0.94`,
    RELEASE_ROOT,
  ]);
});

test("扁平资产 URL 拼到 v 版本目录", () => {
  const urls = buildReleaseAssetUrlCandidates(
    [`${RELEASE_ROOT}/v0.0.94`],
    ["zz-manifest-linux-x64.json"],
  );
  assert.deepEqual(urls, [`${RELEASE_ROOT}/v0.0.94/zz-manifest-linux-x64.json`]);
});

test("版本锁校验同时接受 v 前缀与根目录", () => {
  assert.doesNotThrow(() =>
    assertRemoteCdnBaseVersionMatches([`${RELEASE_ROOT}/v0.0.94`], "0.0.94"),
  );
  assert.doesNotThrow(() => assertRemoteCdnBaseVersionMatches([RELEASE_ROOT], "0.0.94"));
  assert.throws(
    () => assertRemoteCdnBaseVersionMatches([`${RELEASE_ROOT}/v0.0.93`], "0.0.94"),
    /版本不匹配/,
  );
});
