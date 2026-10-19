import assert from "node:assert/strict";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";

import {
  isDistributionDebugFile,
  pruneDistribution,
  shouldPruneTargetDirectory,
} from "./prune.mjs";

function makeTree(files) {
  const root = mkdtempSync(join(tmpdir(), "mode-prune-test-"));
  for (const [relativePath, content] of Object.entries(files)) {
    const full = join(root, relativePath);
    mkdirSync(dirname(full), { recursive: true });
    writeFileSync(full, content);
  }
  return root;
}

test("通用裁剪：只删 source map / PDB / 类型声明，保留运行期文件", async () => {
  const root = makeTree({
    "web/assets/index.js": "js",
    "web/assets/index.js.map": "map",
    "server/entry-http.js": "js",
    "server/entry-http.js.map": "map",
    "agent/mode.cjs": "cjs",
    "agent/node_modules/@larksuiteoapi/node-sdk/types/index.d.ts": "types",
    "node_modules/node-pty/prebuilds/linux-x64/pty.node": "elf",
    "node_modules/node-pty/prebuilds/linux-x64/pty.pdb": "pdb",
    "node_modules/foo/index.tsbuildinfo": "cache",
  });
  try {
    const report = await pruneDistribution(root);
    assert.ok(!existsSync(join(root, "web/assets/index.js.map")));
    assert.ok(!existsSync(join(root, "server/entry-http.js.map")));
    assert.ok(
      !existsSync(join(root, "agent/node_modules/@larksuiteoapi/node-sdk/types/index.d.ts")),
    );
    assert.ok(!existsSync(join(root, "node_modules/node-pty/prebuilds/linux-x64/pty.pdb")));
    assert.ok(!existsSync(join(root, "node_modules/foo/index.tsbuildinfo")));
    assert.ok(existsSync(join(root, "web/assets/index.js")));
    assert.ok(existsSync(join(root, "server/entry-http.js")));
    assert.ok(existsSync(join(root, "agent/mode.cjs")));
    assert.ok(existsSync(join(root, "node_modules/node-pty/prebuilds/linux-x64/pty.node")));
    assert.ok(report.files >= 4);
    assert.ok(report.bytes > 0);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("目标平台裁剪：fnOS 只留 linux-x64 原生件", async () => {
  const root = makeTree({
    "agent/node_modules/@mbears/opentui-core-linux-x64/libopentui.so": "elf",
    "agent/node_modules/@mbears/opentui-core-win32-x64/opentui.dll": "pe",
    "agent/node_modules/koffi/build/koffi/linux_x64/koffi.node": "elf",
    "agent/node_modules/koffi/build/koffi/win32_x64/koffi.node": "pe",
    "node_modules/node-pty/prebuilds/linux-x64/pty.node": "elf",
    "node_modules/node-pty/prebuilds/win32-x64/pty.node": "pe",
    "node_modules/node-pty/third_party/conpty/OpenConsole.exe": "pe",
    "node_modules/node-pty/build/Release/pty.node": "host-build",
    "node_modules/node-pty/build/Release/conpty/conpty.dll": "pe",
  });
  try {
    await pruneDistribution(root, { targetPlatform: "linux", targetArch: "x64" });
    assert.ok(
      existsSync(join(root, "agent/node_modules/@mbears/opentui-core-linux-x64/libopentui.so")),
    );
    assert.ok(!existsSync(join(root, "agent/node_modules/@mbears/opentui-core-win32-x64")));
    assert.ok(existsSync(join(root, "agent/node_modules/koffi/build/koffi/linux_x64/koffi.node")));
    assert.ok(!existsSync(join(root, "agent/node_modules/koffi/build/koffi/win32_x64")));
    assert.ok(existsSync(join(root, "node_modules/node-pty/prebuilds/linux-x64/pty.node")));
    assert.ok(!existsSync(join(root, "node_modules/node-pty/prebuilds/win32-x64")));
    assert.ok(!existsSync(join(root, "node_modules/node-pty/third_party/conpty")));
    assert.ok(!existsSync(join(root, "node_modules/node-pty/build/Release/conpty")));
    // 与本机同平台（Linux）时 build/Release 是有效产物；交叉打包时整档删除。
    const sameAsHost = process.platform === "linux" && process.arch === "x64";
    assert.equal(
      existsSync(join(root, "node_modules/node-pty/build/Release/pty.node")),
      sameAsHost,
    );
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("裁剪判定只匹配已知路径", () => {
  assert.equal(isDistributionDebugFile("web/assets/index.js.map"), true);
  assert.equal(isDistributionDebugFile("web/assets/index.js"), false);
  assert.equal(
    shouldPruneTargetDirectory("agent/node_modules/@mbears/opentui-core-win32-x64", {
      targetPlatform: "linux",
      targetArch: "x64",
    }),
    true,
  );
  assert.equal(
    shouldPruneTargetDirectory("agent/node_modules/@mbears/opentui-core", {
      targetPlatform: "linux",
      targetArch: "x64",
    }),
    false,
  );
  assert.equal(
    shouldPruneTargetDirectory("node_modules/node-pty/prebuilds/linux-x64", {
      targetPlatform: "linux",
      targetArch: "x64",
    }),
    false,
  );
});
