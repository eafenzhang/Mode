// Agent bundle 的暂存动作：C4 默认 Rust 后把 agent-server 作为 mode-agent 放进
// bundled-agents/<平台>/glm，并写 meta；native 缺失时才回退 stage mode.cjs。
//
// dev 与打包**必须**用同一份暂存实现。
// 只有打包链（prepare-agent-node-bundle.mjs）会暂存是不够的，dev 链
// （scripts/build-desktop-agent-cli.mjs）不会；而 dev 未打包时的 agent 二进制由
// desktopRuntimeEnv.ts 的 resolveBundledModeAgentBinaryPath() 解析，候选**只有**
// bundled-agents/，没有 cli/dist/。于是 dev 一直跑着上一次打包时留下的那份 ——
// 实测陈旧 3 天，任何 agent CLI 侧改动在 dev 里静默不生效，排查时会把「改动没生效」
// 误判成「代码没起作用」。两边共用这一份，dev 与打包不可能再各自漂移。
import { chmodSync, copyFileSync, existsSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { isAbsolute, resolve } from "node:path";

export const AGENT_BUNDLE_SOURCE_RELATIVE = "apps/mode-cli/packages/cli/dist/mode.cjs";

export function resolveAgentBundlePaths({ repoRoot, platformKey }) {
  const glmDir = resolve(repoRoot, "packages", "desktop", "bundled-agents", platformKey, "glm");
  return {
    cliBundlePath: resolve(repoRoot, AGENT_BUNDLE_SOURCE_RELATIVE),
    glmDir,
    stagedBundlePath: resolve(glmDir, "mode.cjs"),
    stagedMetaPath: resolve(glmDir, ".node-bundle-meta.json"),
  };
}

/**
 * 干净重建 glm 目录再拷贝。清空是刻意的：Tauri 发运集整目录拷贝
 * bundled-agents/<平台>/glm → resources/agent，本地工作树里上一次构建残留的原生二进制
 * （mode-agent / mode-acp 等）和旧 meta 会被一并打进安装包（CI 干净检出不会有，本地会）。
 */
/**
 * 默认运行时翻转（C4）：把 Rust `agent-server` 作为 `mode-agent` 一起放进 glm。
 *
 * 来源优先级：`MODE_AGENT_NATIVE_BINARY` → `target/agent-release|release|debug/agent-server[.exe]`。
 * 找不到就只保留 Node bundle（宿主会回退 mode.cjs）。dev 先 `cargo build -p agent-server`
 * 即可让随包/本地解析命中 Rust；发布链在 tauri build 前显式构建并传 env。
 */
export function stageNativeAgentBinary({ repoRoot, glmDir, platformKey, log = console.log }) {
  const windows = platformKey.startsWith("win32");
  const binaryName = windows ? "mode-agent.exe" : "mode-agent";
  const serverName = windows ? "agent-server.exe" : "agent-server";
  const configured = process.env.MODE_AGENT_NATIVE_BINARY?.trim();
  const candidates = [
    configured ? (isAbsolute(configured) ? configured : resolve(repoRoot, configured)) : null,
    resolve(repoRoot, "target", "agent-release", serverName),
    resolve(repoRoot, "target", "release", serverName),
    resolve(repoRoot, "target", "debug", serverName),
  ].filter(Boolean);
  for (const candidate of candidates) {
    if (!existsSync(candidate)) continue;
    const target = resolve(glmDir, binaryName);
    copyFileSync(candidate, target);
    if (!windows) chmodSync(target, 0o755);
    log(`[stage:agent-bundle] staged native ${target} <- ${candidate}`);
    return target;
  }
  log("[stage:agent-bundle] native agent-server 未找到；保留 Node bundle 回退");
  return null;
}

export function stageAgentBundle({ repoRoot, platformKey, log = console.log }) {
  const { cliBundlePath, glmDir, stagedBundlePath, stagedMetaPath } = resolveAgentBundlePaths({
    repoRoot,
    platformKey,
  });
  rmSync(glmDir, { recursive: true, force: true });
  mkdirSync(glmDir, { recursive: true });
  const nativeBinaryPath = stageNativeAgentBinary({ repoRoot, glmDir, platformKey, log });
  // 默认 Rust：native 存在时不再随包 mode.cjs（省掉 ~16.5MB 的 Node bundle）；
  // 仅 native 缺失的 dev/异常场景保留 mode.cjs 回退。
  if (!nativeBinaryPath) {
    if (!existsSync(cliBundlePath)) {
      throw new Error(`[stage:agent-bundle] agent bundle 源产物不存在：${cliBundlePath}`);
    }
    copyFileSync(cliBundlePath, stagedBundlePath);
  }
  const nativeName = platformKey.startsWith("win32") ? "mode-agent.exe" : "mode-agent";
  const meta = {
    runtime: nativeBinaryPath ? "rust" : "node",
    entry: nativeBinaryPath ? nativeName : "mode.cjs",
    ...(nativeBinaryPath
      ? { nativeBinary: nativeName }
      : { fallbackEntry: "mode.cjs", source: AGENT_BUNDLE_SOURCE_RELATIVE }),
    platform: platformKey,
  };
  writeFileSync(stagedMetaPath, `${JSON.stringify(meta, null, 2)}\n`, "utf8");
  if (!nativeBinaryPath) log(`[stage:agent-bundle] staged ${stagedBundlePath}`);
  return { stagedBundlePath, stagedMetaPath, nativeBinaryPath };
}
