import type { DatabaseStartupState } from "@mode/shared";

/**
 * Host 传输死亡（post-boot）判定（docs/specs/tauri-renderer-host.md §22）。
 *
 * 背景（2026-10-25 验收缺陷）：Host 进程死亡后，壳与注入桥都会如实上报
 * `phase: "failed"`，但渲染层在启动完成后把状态流静默丢弃，`ChannelClient`
 * 永不 dispose——MessagePort 对端死亡不产生事件，于是全部服务 RPC 永久 pending。
 *
 * 判据（三条同时成立才收口）：
 * - `appInitialized`：启动准入（port + ready 同代）已完成，状态流从此只可能是
 *   运行态事实；boot 期失败仍走既有 `GlobalDatabaseStartupLoading` 准入路径；
 * - `state.phase === "failed"`：壳侧 `read_loop` 对 stdout EOF 发布的唯一终态；
 * - `state.startupId` 与本窗口**准入过**的 startupId 同代：换代保护——异代
 *   （旧实例遗留/他窗状态）不得误杀本窗连接。
 */
export function isPostBootHostTransportDeath(
  state: DatabaseStartupState,
  params: {
    appInitialized: boolean;
    admittedStartupId: string | null;
    alreadyHandled: boolean;
  },
): boolean {
  if (params.alreadyHandled || !params.appInitialized) {
    return false;
  }
  if (state.phase !== "failed") {
    return false;
  }
  if (!params.admittedStartupId || state.startupId !== params.admittedStartupId) {
    return false;
  }
  return true;
}
