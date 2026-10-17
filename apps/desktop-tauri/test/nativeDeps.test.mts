/**
 * M3 Task 2 · Step 1（DR-T2-1）：**原生依赖在 sidecar 里是否可用**。
 *
 * 为什么这条用例必须最先存在：DR-M3-1 选 Node sidecar 的**唯一理由**是复用存量
 * `packages/services`，而那一层依赖两类 ABI 敏感的原生资产——`node:sqlite`（`DatabaseSync`，
 * 需要较新 Node）与 `node-pty`（原生模块，Windows 上走 conpty）。若它们在 sidecar 的运行环境里
 * 加载不了，Node sidecar 的前提就不成立，要回到 DR-M3-1 重新决策（那是比 Task 2 大一个量级的改动）。
 * 所以这件事的代价是「判错就要重开决策」，它必须在最前面而不是在接了二十个服务之后才发现。
 *
 * 两层证据，缺一不可：
 *
 * 1. **本进程**（与 sidecar 同一个 `tsx` loader、同一个安装）：sqlite 真建表/写/读、pty 模块与
 *    API 面与原生 artifact 都在。这一层**不真起 pty**——Windows 上 node-pty 第一次真起进程会
 *    建一个常驻的 conout worker 线程（`MessagePort`），在本进程里它是不会退出的，
 *    会把测试进程挂住（实测：不限时的话跑满超时）。所以真起进程放到第 2 层。
 * 2. **sidecar 进程内**：起生产 Host 入口（壳 spawn 的那份）并让它自己探测，`host.describe`
 *    把结果报回来。测试把 `MODE_HOST_NATIVE_DEP_PROBE=spawn` 设给子进程，于是**真起一个 pty**
 *    这条强证据发生在 sidecar 进程里；那个 worker 线程随 sidecar 一起被收掉（Host 的 stdin EOF
 *    保护会 `process.exit`），不会影响测试进程。
 *
 * CUA 是 opt-in 内测特性：它的原生资产经 env 注入（`MODE_CUA_BUNDLED_HELPER_APP_PATH`，
 * 见 `packages/services/src/node.ts` 的 `resolveBundledCuaHelperAppPath`）。**未注入不是失败**
 * （缺省语义是「本机不提供 CUA」），但**注入了不存在的路径必须是失败**——否则「配置写错」
 * 会被读成「CUA 正常但没启用」。
 */
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import assert from "node:assert/strict";
import test from "node:test";

import {
  connectProductionHost,
  describeHost,
  disposeHost,
  removeSandbox,
} from "./fixtures/productionHost.mts";
import {
  MODE_CUA_BUNDLED_HELPER_APP_PATH_ENV,
  probeNativeDeps,
} from "../../../packages/desktop/src/host-standalone/nativeDeps.js";

test("原生依赖在 sidecar 的 loader 下可加载：node:sqlite 真读写、node-pty 模块与原生 artifact 到位", () => {
  const report = probeNativeDeps();
  assert.equal(
    report.sqlite.ok,
    true,
    `node:sqlite 必须可用（DR-T2-1 的前提）：${JSON.stringify(report.sqlite)}`,
  );
  // 「可用」不是「import 没抛错」：真写进去的值必须真读得回来，版本来自 sqlite_version()。
  assert.match(report.sqlite.version ?? "", /^\d+\.\d+/, "sqlite 版本来自 sqlite_version()");
  assert.equal(report.sqlite.roundTrip, 41, "写进去的值必须真读得回来");
  assert.equal(
    report.pty.ok,
    true,
    `node-pty 必须可用（真实 terminalService 依赖它）：${JSON.stringify(report.pty)}`,
  );
  assert.ok(
    (report.pty.nativeArtifacts ?? []).length > 0,
    `必须找到原生 .node artifact：${JSON.stringify(report.pty)}`,
  );
});

test("CUA 资产路径：未注入不是失败，注入了不存在的路径必须是失败", () => {
  const absent = probeNativeDeps({});
  assert.equal(absent.cua.injected, false, "没设 env 时如实报未注入");
  assert.equal(absent.cua.ok, false, "未注入时 ok=false，但它不代表失败（opt-in 特性缺省关闭）");

  const bogus = join(tmpdir(), "mode-cua-does-not-exist", "helper.exe");
  const injected = probeNativeDeps({ [MODE_CUA_BUNDLED_HELPER_APP_PATH_ENV]: bogus });
  assert.equal(injected.cua.injected, true);
  assert.equal(
    injected.cua.ok,
    false,
    "注入了不存在的路径必须判失败——否则「配置写错」会被读成「CUA 正常但没启用」",
  );
  assert.equal(injected.cua.path, bogus, "报告里必须带上被判失败的那个路径");
});

/**
 * Ruling 20：Tauri 产品本阶段**只出 Windows**。这条用例的强证据是 sidecar 进程里真起一个
 * pty（Windows 上走 conpty）；在 Linux/macOS 上跑既不代表产品形态，也不能把平台差异
 * 读成产品缺陷。非 Windows 必须**显式 skip 并给出理由**，而不是静默通过——
 * 阻断发布的 Windows 腿在 `.github/workflows/release.yml` 的 `tauri` job 里跑这条套件。
 */
const nonWindowsSkip =
  process.platform === "win32"
    ? false
    : `仅 Windows 可跑（Ruling 20：Tauri 产品本阶段只出 Windows；当前平台 ${process.platform}）`;

test(
  "生产 Host sidecar 自己报告原生依赖可用，并真起一个 pty（同一进程内的证据）",
  { skip: nonWindowsSkip },
  async () => {
    const sandbox = mkdtempSync(join(tmpdir(), "mode-host-native-"));
    const connection = await connectProductionHost({
      home: join(sandbox, "home"),
      dataBaseDir: join(sandbox, "data"),
      extraEnv: { MODE_HOST_NATIVE_DEP_PROBE: "spawn" },
    });
    try {
      const describe = await describeHost(connection);
      const native = describe.nativeDeps;
      assert.ok(native, "host.describe 必须报告 sidecar 进程内的原生依赖探测结果");
      assert.equal(
        native.sqlite.ok,
        true,
        `sidecar 内 node:sqlite：${JSON.stringify(native.sqlite)}`,
      );
      assert.equal(native.pty.ok, true, `sidecar 内 node-pty：${JSON.stringify(native.pty)}`);
      assert.equal(native.pty.spawnVerified, true, "深探测档必须真起过 pty 并验证");
      assert.equal(native.pty.exitCode, 0, `sidecar 内 pty 退出码：${JSON.stringify(native.pty)}`);
      assert.match(native.pty.output ?? "", /pty-ok/, "sidecar 内 pty 的输出必须真的送回来");
    } finally {
      await disposeHost(connection);
      await removeSandbox(sandbox);
    }
  },
);
