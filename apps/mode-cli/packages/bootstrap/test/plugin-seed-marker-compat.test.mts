import assert from "node:assert/strict";
import test from "node:test";

import {
  OFFICIAL_PLUGIN_SEED_MARKER_FILE,
  resolveOfficialPluginSeedMarkerFileName,
} from "../src/app/official-plugin-seed-marker.js";

test("seed 标记文件名：新名优先，旧标记仍可判定为已 seed", () => {
  assert.equal(OFFICIAL_PLUGIN_SEED_MARKER_FILE, ".tutto-plugin-seed.json");
  assert.equal(
    resolveOfficialPluginSeedMarkerFileName((name) => name === ".tutto-plugin-seed.json"),
    ".tutto-plugin-seed.json",
  );
  assert.equal(
    resolveOfficialPluginSeedMarkerFileName((name) => name === ".zcode-plugin-seed.json"),
    ".zcode-plugin-seed.json",
  );
  assert.equal(
    resolveOfficialPluginSeedMarkerFileName(() => false),
    ".tutto-plugin-seed.json",
  );
});
