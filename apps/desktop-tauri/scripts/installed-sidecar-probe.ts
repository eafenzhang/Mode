/**
 * 「用**安装目录里的** node + sidecar 真开一个 pty」这件事的探针（M3 Task 5）。
 *
 * 抽出来的理由与 `installed-artifacts.ts` 相同（`verify-installed.ts` 撞了 `max-lines`）。
 * 这一段与「起壳跑一次」那条链的区别是它有**自己的进程**（随包 node 起安装目录里的
 * `index.mjs`），所以要管自己的收摊顺序。
 *
 * 两条判定的来历（都是实测踩出来的）：
 *
 * 1. **`describe.nativeDeps.pty.nativeArtifacts` 只在加载档产出**（深探测档换成
 *    `spawned`/`exitCode`/`output`）。第一版在深探测档里断言那个字段，于是把「字段本来就不产出」
 *    读成了「artifact 不在」。**报文里没有的东西不该被断言**，所以 artifact 的存在性直接量文件系统，
 *    而「绑定真的可用」交给深探测档那一次真起进程（win32 上 node-pty 没有「加载绑定」的公开入口，
 *    「模块在」证不了它可用）。
 * 2. **`IChannel.call(command, arg)` 的第二参在服务端是位置参数数组**
 *    （`target.apply(handler, args || [])`），所以裸对象等于**一个参数都不传**：
 *    服务收到 `undefined` 并当场抛 `TypeError: Cannot read properties of undefined (reading 'cwd')`
 *    （第一版的真实症状）。契约在 `packages/services/src/terminal/terminal.ts` 的接口里，
 *    **类型系统拦不住**——那里是 `IChannel`。
 */
import { existsSync, mkdirSync, readdirSync } from "node:fs";
import { join } from "node:path";

import type { IChannel } from "@mode/rpc";

import { connectToSidecar } from "./sidecar-stdio-client.ts";
import type { CheckFn } from "./installed-artifacts.ts";

export interface TerminalProbeResult {
  shell: string;
  output: string;
  shippedArtifacts: string[];
}

/**
 * 随包 node-pty 的**原生 artifact 在不在**（看文件系统，不看探针报告）。
 *
 * 同时断言非目标平台的预编译件**已经不在**——那是剪枝真正要证的命题（只报体积不算验过）；
 * 并断言 `build/Release/conpty` **没被连带裁掉**（实测：裁了它 pty 就起不来）。
 */
export function checkShippedPtyArtifacts(installDir: string, check: CheckFn): string[] {
  const ptyRoot = join(installDir, "host-sidecar", "node_modules", "node-pty");
  const listNodeFiles = (dir: string): string[] =>
    existsSync(dir)
      ? readdirSync(dir)
          .filter((name) => name.endsWith(".node"))
          .map((name) => join(dir, name))
      : [];
  const win64 = listNodeFiles(join(ptyRoot, "prebuilds", "win32-x64"));
  check(
    "发运集里有目标平台的 node-pty 原生 artifact（win32-x64）",
    win64.length > 0,
    win64.length > 0 ? win64.join(", ") : `${join(ptyRoot, "prebuilds", "win32-x64")} 下没有 .node`,
  );
  const prunedAway = ["win32-arm64", "darwin-arm64", "darwin-x64"].filter((dir) =>
    existsSync(join(ptyRoot, "prebuilds", dir)),
  );
  check(
    "剪枝确实把非目标平台的 prebuilds 裁掉了",
    prunedAway.length === 0,
    prunedAway.length === 0
      ? "win32-arm64 / darwin-* 都不在发运集里"
      : `仍然存在：${prunedAway.join(", ")}`,
  );
  const conpty = join(ptyRoot, "build", "Release", "conpty");
  check("conpty 那条路要用的 build/Release/conpty 没被连带裁掉", existsSync(conpty), conpty);
  return win64;
}

/** 用**安装目录里的** node + sidecar 真开一个 pty（剪枝是否把运行时弄坏了，由这一条回答）。 */
export async function probeInstalledTerminal(options: {
  installDir: string;
  workDir: string;
  nonce: string;
  /** 新开 pty 的工作目录（用仓库根：终端要有个真实存在的 cwd）。 */
  ptyCwd: string;
  check: CheckFn;
}): Promise<TerminalProbeResult> {
  const { installDir, workDir, nonce, ptyCwd, check } = options;
  const node = join(installDir, "node", "node.exe");
  const entry = join(installDir, "host-sidecar", "index.mjs");
  const sandbox = join(workDir, "sidecar-sandbox");
  mkdirSync(sandbox, { recursive: true });
  const connection = await connectToSidecar({
    node,
    args: [entry],
    cwd: installDir,
    env: {
      ...process.env,
      HOME: sandbox,
      USERPROFILE: sandbox,
      MODE_DATA_BASE_DIR: join(sandbox, "data"),
      MODE_HOST_STARTUP_ID: "installed-verify",
      MODE_HOST_RESOURCES_DIR: installDir,
      // 深探测档：sidecar 启动过程真起一个 pty 跑一条命令（默认档不设，见 nativeDeps.ts）。
      MODE_HOST_NATIVE_DEP_PROBE: "spawn",
      MODE_BUILTIN_PROVIDER_CONFIG_FILE: "",
    },
    readyTimeoutMs: 60_000,
  });
  try {
    const host = connection.client.getChannel<IChannel>("host");
    const describe = await host.call<{
      nodeVersion: string;
      dataRoot: string;
      serviceSurface: {
        kind: string;
        builtinProviderConfigSource: string;
        builtinProviderConfigExists: boolean;
      };
      nativeDeps: {
        sqlite: { ok: boolean; version?: string; roundTrip?: number };
        pty: {
          ok: boolean;
          spawned?: boolean;
          spawnVerified?: boolean;
          exitCode?: number;
          output?: string;
          error?: string;
        };
      };
    }>("describe");
    check(
      "安装目录里的 sidecar 服务面",
      describe.serviceSurface.kind === "services",
      `kind=${describe.serviceSurface.kind}，Node ${describe.nodeVersion}`,
    );
    check(
      "随包 Node + 资源目录档的 Built-in Provider Config",
      describe.serviceSurface.builtinProviderConfigSource === "resources" &&
        describe.serviceSurface.builtinProviderConfigExists,
      `source=${describe.serviceSurface.builtinProviderConfigSource} exists=${String(describe.serviceSurface.builtinProviderConfigExists)}`,
    );
    const shippedArtifacts = checkShippedPtyArtifacts(installDir, check);
    check(
      "`node:sqlite` 真读写",
      describe.nativeDeps.sqlite.ok,
      `sqlite_version=${String(describe.nativeDeps.sqlite.version)} roundTrip=${String(describe.nativeDeps.sqlite.roundTrip)}`,
    );
    // 剪枝后 node-pty **真能用**：深探测档在 sidecar 进程里起了一个 pty 跑 `cmd /c echo pty-ok`，
    // 断言退出码为 0 且输出里真有那个标记。
    const pty = describe.nativeDeps.pty;
    check(
      "深探测档真起过 pty 并跑出一条命令（剪枝没弄坏 node-pty）",
      pty.spawned === true && pty.exitCode === 0 && (pty.output ?? "").includes("pty-ok"),
      `spawned=${String(pty.spawned)} exitCode=${String(pty.exitCode)} output=${JSON.stringify(pty.output ?? null)}` +
        ` error=${String(pty.error)}`,
    );

    // 端到端：走 `terminal` 频道自己建一个 pty、写一条命令、从事件流里读回标记。
    const terminal = connection.client.getChannel<IChannel>("terminal");
    const created = await terminal.call<{
      id: string;
      shell: string;
      windowsPty?: { backend: string; buildNumber?: number };
    }>("create", [{ cols: 120, rows: 30, cwd: ptyCwd }]);
    const marker = `MODE_PTY_OK_${nonce}`;
    let output = "";
    const received = new Promise<void>((resolveMarker) => {
      terminal.listen<string>(
        "onDynamicData",
        created.id,
      )((chunk) => {
        output += chunk;
        if (output.includes(marker)) resolveMarker();
      });
    });
    await terminal.call("write", [{ id: created.id, data: `echo ${marker}\r\n` }]);
    await Promise.race([received, new Promise((r) => setTimeout(r, 20_000))]);
    await terminal.call("dispose", [{ id: created.id }]);
    check(
      "`terminal` 频道端到端：建 pty + 写命令 + 读回输出",
      output.includes(marker),
      `shell=${created.shell} backend=${created.windowsPty?.backend ?? "?"}；输出 ${output.length} 字符` +
        `（含标记=${String(output.includes(marker))}）`,
    );
    return { shell: created.shell, output, shippedArtifacts };
  } finally {
    await connection.dispose();
  }
}
