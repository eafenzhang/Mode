/**
 * 发运集与剪枝规则的回归网（M3 Task 5）。
 *
 * 两条本任务实测踩到的坑都在这里钉住（都不是假想题）：
 *
 * 1. **目录路径不带尾斜杠**，所以 `prebuilds/win32-x64` 自己也要被判进去；
 * 2. 更隐蔽的一条：**裁掉父目录会连带吞掉例外**——第一版对 `prebuilds` 本体也返回「裁」，
 *    于是 `win32-x64` 从没被判定过，整个 `prebuilds` 消失（实测：node-pty 只剩 6 MB，
 *    比应有的 ~34 MiB 少 29 MiB）。
 *
 * 用**合成的包树**测，不依赖本 checkout 的 `node_modules` 内容：这层要证的是「规则与拷贝
 * 用的是同一套判定」，而真实包树的内容会随锁文件变动。
 */
import assert from "node:assert/strict";
import { existsSync, mkdirSync, mkdtempSync, rmSync, statSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

import type { ExternalClosure } from "../scripts/host-sidecar-ship.ts";

/**
 * 用 `createRequire` 而不是 `import`：本包的 `package.json` **没有** `"type": "module"`，
 * 所以 tsx 把 `scripts/*.ts` 当 CJS 加载，而本文件是 `.mts`（ESM）——ESM 侧看不到 CJS 的具名
 * 导出（实测症状：`does not provide an export named 'isPrunedOut'`）。本仓库既有的用例
 * 只 import `.mts` 夹具，所以没有先例可抄；`require` 这一档拿到的是同一份实现
 * （`typeof import(...)` 只用来取类型）。
 */
const require_ = createRequire(import.meta.url);
const { isPrunedOut, stageShipSet } = require_(
  "../scripts/host-sidecar-ship.ts",
) as typeof import("../scripts/host-sidecar-ship.ts");

/** 造一棵最小的假包树：`node-pty` 的形状（prebuilds 三档 + 非 prebuilds 资产）。 */
function makeFakeNodePty(root: string): string {
  const packageDir = join(root, "node-pty");
  const files = [
    "lib/index.js",
    "build/Release/conpty/conpty.dll",
    "prebuilds/win32-x64/conpty.node",
    "prebuilds/win32-arm64/conpty.node",
    "prebuilds/darwin-arm64/pty.node",
  ];
  for (const relative of files) {
    const full = join(packageDir, relative);
    mkdirSync(join(full, ".."), { recursive: true });
    writeFileSync(full, "x".repeat(16));
  }
  mkdirSync(join(packageDir, "node_modules/hoisted"), { recursive: true });
  writeFileSync(join(packageDir, "node_modules/hoisted/index.js"), "y".repeat(1024));
  return packageDir;
}

function makeClosure(packageDir: string): ExternalClosure {
  return {
    roots: [{ name: "node-pty", version: "1.1.0", packageDir, bytes: 0 }],
    packages: [{ name: "node-pty", version: "1.1.0", packageDir, bytes: 0 }],
    unresolved: [],
    collisions: [],
  };
}

function stageFixture(root: string, variant: "core" | "pruned" | "unpruned"): string {
  const packageDir = makeFakeNodePty(root);
  const bundleFile = join(root, "bundle.mjs");
  writeFileSync(bundleFile, "export {};\n");
  const nodeSource = join(root, "fake-node.exe");
  writeFileSync(nodeSource, "z".repeat(64));
  const configSource = join(root, "mode-builtin.json");
  writeFileSync(configSource, "{}\n");
  const stageRoot = join(root, `ship-${variant}`);
  stageShipSet({
    variant,
    bundleFile,
    stageRoot,
    nodeSource,
    builtinProviderConfigSource: configSource,
    closure: makeClosure(packageDir),
  });
  return stageRoot;
}

test("剪枝：只留 win32-x64，且父目录本身不能连带被裁", () => {
  assert.equal(isPrunedOut("node-pty", "prebuilds"), false, "父目录必须保留，否则例外无从判定");
  assert.equal(isPrunedOut("node-pty", "prebuilds/win32-x64"), false, "目标平台的目录本体要留");
  assert.equal(isPrunedOut("node-pty", "prebuilds/win32-x64/conpty.node"), false);
  assert.equal(isPrunedOut("node-pty", "prebuilds/win32-arm64"), true, "非目标架构整档裁掉");
  assert.equal(isPrunedOut("node-pty", "prebuilds/darwin-arm64/pty.node"), true);
  assert.equal(isPrunedOut("node-pty", "lib/index.js"), false, "prebuilds 之外一律不动");
  // conpty 那条路要用 build/Release/conpty（实测：裁了它 pty 就起不来）。
  assert.equal(isPrunedOut("node-pty", "build/Release/conpty/conpty.dll"), false);
  assert.equal(isPrunedOut("@types/node", "index.d.ts"), true, "纯类型包整个不发");
  assert.equal(isPrunedOut("ssh2", "prebuilds/win32-arm64"), false, "规则只对写了规则的那个包生效");
});

/**
 * Ruling 20：发运集本阶段只出 `win32-x64`，且 `copyNodeRuntime` 在非 Windows 上写的是
 * `node` 而不是产品契约里的 `node/node.exe`。因此这条「真实落点」断言只在 Windows 成立；
 * 非 Windows 显式 skip 并说明理由，阻断发布的 Windows 腿在 release.yml 的 `tauri` job 里跑。
 */
const nonWindowsSkip =
  process.platform === "win32"
    ? false
    : `仅 Windows 可跑（Ruling 20：发运集只出 win32-x64；当前平台 ${process.platform} 的随包运行时名为 node）`;

test(
  "发运集：剪枝档留下 win32-x64、丢掉别的平台，且不拷嵌套 node_modules",
  { skip: nonWindowsSkip },
  () => {
    const root = mkdtempSync(join(tmpdir(), "mode-ship-test-"));
    try {
      const stageRoot = stageFixture(root, "pruned");
      const vendor = join(stageRoot, "host-sidecar", "node_modules", "node-pty");
      assert.ok(existsSync(join(vendor, "prebuilds", "win32-x64", "conpty.node")));
      assert.ok(!existsSync(join(vendor, "prebuilds", "win32-arm64")), "非目标架构不应进发运集");
      assert.ok(!existsSync(join(vendor, "prebuilds", "darwin-arm64")));
      assert.ok(
        !existsSync(join(vendor, "node_modules")),
        "嵌套 node_modules 由闭包单独搬运，不该在这一层",
      );
      assert.ok(existsSync(join(vendor, "build", "Release", "conpty", "conpty.dll")));
      // 三个落点都要在（壳按它们找人）：bundle、随包 Node、Built-in Provider Config。
      assert.ok(existsSync(join(stageRoot, "host-sidecar", "index.mjs")));
      assert.ok(existsSync(join(stageRoot, "node", "node.exe")));
      assert.ok(existsSync(join(stageRoot, "config", "provider", "mode-builtin.json")));
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  },
);

test("发运集：core 档不带 Node 与 externals，但仍创建那两个空目录", () => {
  const root = mkdtempSync(join(tmpdir(), "mode-ship-test-"));
  try {
    const stageRoot = stageFixture(root, "core");
    assert.ok(existsSync(join(stageRoot, "host-sidecar", "index.mjs")));
    // 空目录：`bundle.resources` 的路径在三档之间保持不变，靠的就是它们存在（空目录会被跳过）。
    assert.ok(existsSync(join(stageRoot, "node")));
    assert.equal(statSync(join(stageRoot, "node")).isDirectory(), true);
    assert.ok(!existsSync(join(stageRoot, "node", "node.exe")));
    assert.ok(!existsSync(join(stageRoot, "host-sidecar", "node_modules", "node-pty")));
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
