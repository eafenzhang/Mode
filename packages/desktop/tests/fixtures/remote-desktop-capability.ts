/**
 * 远程桌面共享位夹具（spec: docs/specs/remote-desktop-system-rdp.md 产品规则 5/7）：
 * 口径 = win32 && 打包层 kill-switch 未触发 && 提示性共享开关开（**不是**安全闸门，拦不住 3389）。纯函数断言，跨平台可跑
 * （组合逻辑不依赖真实 process.platform，故 macOS/Linux CI 也能验证 win32 分支）。
 * 失效模式（未实现时）：非 win32 放行 / kill-switch 压不住 / 开关关仍报支持 → 断言失败。
 */
import assert from "node:assert/strict";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

// services 在模块加载时读 MODE_DATA_BASE_DIR，必须先设再动态导入。
process.env.MODE_DATA_BASE_DIR = await mkdtemp(join(tmpdir(), "mode-rd-capability-"));
const {
  MODE_REMOTE_DESKTOP_KILL_SWITCH_ENV_KEY,
  computeRemoteDesktopCapability,
  isRemoteDesktopKillSwitchEngaged,
} = await import("../../src/host/lanAccessServer.js");

// ── kill-switch：只能关不能开 ──
assert.equal(isRemoteDesktopKillSwitchEngaged({}), false, "未设置不得触发 kill-switch");
assert.equal(
  isRemoteDesktopKillSwitchEngaged({ [MODE_REMOTE_DESKTOP_KILL_SWITCH_ENV_KEY]: "0" }),
  false,
  "0 不得触发",
);
assert.equal(
  isRemoteDesktopKillSwitchEngaged({ [MODE_REMOTE_DESKTOP_KILL_SWITCH_ENV_KEY]: "off" }),
  false,
  "off 不得触发",
);
assert.equal(
  isRemoteDesktopKillSwitchEngaged({ [MODE_REMOTE_DESKTOP_KILL_SWITCH_ENV_KEY]: "1" }),
  true,
  "1 触发",
);
assert.equal(
  isRemoteDesktopKillSwitchEngaged({ [MODE_REMOTE_DESKTOP_KILL_SWITCH_ENV_KEY]: " TRUE " }),
  true,
  "取值大小写与空白不敏感",
);
assert.equal(
  isRemoteDesktopKillSwitchEngaged({ [MODE_REMOTE_DESKTOP_KILL_SWITCH_ENV_KEY]: "on" }),
  true,
  "on 触发",
);

// ── 组合口径 ──
assert.equal(
  computeRemoteDesktopCapability({
    platform: "win32",
    killSwitchEngaged: false,
    serverEnabled: true,
  }),
  true,
  "win32 + 开关开 + kill-switch 未触发 = 支持",
);
assert.equal(
  computeRemoteDesktopCapability({
    platform: "win32",
    killSwitchEngaged: true,
    serverEnabled: true,
  }),
  false,
  "kill-switch 触发：即使开关开也必须压死",
);
assert.equal(
  computeRemoteDesktopCapability({
    platform: "win32",
    killSwitchEngaged: false,
    serverEnabled: false,
  }),
  false,
  "被控开关默认关 = 不支持",
);
assert.equal(
  computeRemoteDesktopCapability({
    platform: "darwin",
    killSwitchEngaged: false,
    serverEnabled: true,
  }),
  false,
  "非 win32 对端一律不支持（macOS/Linux 非目标）",
);
assert.equal(
  computeRemoteDesktopCapability({
    platform: "linux",
    killSwitchEngaged: false,
    serverEnabled: true,
  }),
  false,
  "非 win32 对端一律不支持",
);
console.log("REMOTE_DESKTOP_CAPABILITY_OK");
