import type { RemoteTarget } from "@mode/shared";

/**
 * 按 target 判定两个远程连接描述是否指向同一对端。
 *
 * 消费方（main 侧所有按 target 匹配路由的查询，三处共用同一口径）：
 * 1. hasRemoteWorkspaceSessionForTarget——Bot 桥的连接状态查询；
 * 2. createBotRemoteWorkspaceRuntimePort——绑定/入站消息建立 bot 流观看时的 runtime attachment；
 * 3. reconnectBotRemoteWorkspaceSession——断线后复用已有 session 还是新建。
 *
 * 修复依据（docs/specs/im-bot-remote-workspace-binding.md）：本函数曾缺 lan 分支、
 * 残留 RemoteTarget 中不存在的 server 分支——两个完全相同的 lan target 从 switch
 * 掉出返回 undefined 判否，LAN 远程工作区的连接状态永远「未连接」、bot runtime port
 * 永远「未找到可供 Bot attachment 的远程 logical session」，bindBotToTask 必抛且
 * 绑定菜单无 catch，表现为「远程工作区无法绑定 IM 机器人」。
 *
 * 只比较定位对端所需的 authority 字段；secret（token/密码）与展示性字段
 * （serverName、sshConfigAlias）不参与比较——settings 快照与连接期 live target
 * 的 secret 形态不同，比较它们必然把同一对端误判为不同 target。
 */
export function isSameRemoteTarget(left: RemoteTarget, right: RemoteTarget): boolean {
  if (left.kind !== right.kind) return false;
  switch (left.kind) {
    case "ssh":
      return (
        right.kind === "ssh" &&
        left.host.trim().toLowerCase() === right.host.trim().toLowerCase() &&
        (left.port ?? 22) === (right.port ?? 22) &&
        left.username.trim() === right.username.trim() &&
        (left.privateKeyPath ?? "") === (right.privateKeyPath ?? "")
      );
    case "wsl":
      return (
        right.kind === "wsl" &&
        (left.distro?.trim() || "default") === (right.distro?.trim() || "default") &&
        (left.user?.trim() ?? "") === (right.user?.trim() ?? "")
      );
    case "docker":
      return right.kind === "docker" && left.container === right.container;
    case "lan":
      return (
        right.kind === "lan" &&
        left.host.trim().toLowerCase() === right.host.trim().toLowerCase() &&
        left.port === right.port
      );
  }
}
