import { attachV3WorkspaceSession } from "@mode/client";
import { WebControlLink, importWebIdentity } from "@mode/remote-core/web";
import type { V3ConnectOptions } from "@mode/shared";
import { registerRemoteWorkspaceSession } from "@mode/ui";

import { loadStoredV3Profile } from "./storedProfile.js";

/**
 * 主 Web 的 v3 远程工作区入口：复用 `/v3` 已存的账号会话建 Noise 连接，再把
 * `/ws/host` 隧道注册成远程 session。之后的 tab/文件/终端/Git 与 LAN 路径共用同一 store。
 *
 * 失败语义见 docs/specs/remote-control-v3-web-workspace.md §5。
 */
export async function connectV3RemoteWorkspace(
  options: V3ConnectOptions,
): Promise<{ success: true; sessionId: string } | { success: false; error: string }> {
  const profile = await loadStoredV3Profile();
  const accessToken = profile?.accessToken;
  if (!profile || !accessToken) {
    return { success: false, error: "v3_login_required" };
  }
  const link = new WebControlLink({
    serverUrl: profile.serverUrl,
    deviceId: profile.deviceId,
    accessToken,
    identity: importWebIdentity(profile.privateKey),
  });
  try {
    await link.start();
    await link.connect(options.deviceId);
    const session = await attachV3WorkspaceSession({
      link,
      hostDeviceId: options.deviceId,
      register: (attachment) => {
        registerRemoteWorkspaceSession({
          sessionId: attachment.sessionId,
          target: options,
          services: attachment.services,
          dispose: () => {
            attachment.dispose();
            link.close();
          },
        });
      },
    });
    return { success: true, sessionId: session.sessionId };
  } catch (error) {
    link.close();
    return {
      success: false,
      error: error instanceof Error ? error.message : String(error),
    };
  }
}
