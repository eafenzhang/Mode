import type { ICredentialService } from "@mode/services";
import type { RemoteV3PairingClaim, RemoteV3PairingState } from "@mode/shared";

import { isRemoteV3AuthError } from "./api.js";
import {
  cancelRemoteV3Pairing,
  claimRemoteV3Pairing,
  getRemoteV3Pairing,
  redeemRemoteV3Pairing,
} from "./apiPairing.js";
import { saveRemoteV3ClientTokens, type RemoteV3ClientProfile } from "./identity.js";

/**
 * 桌面 Client 角色的配对方法（docs/specs/remote-control-v3-pairing.md）。
 *
 * 独立成模块以控制 `client.ts` 体量；凭证与刷新仍由 runtime 负责，这里只做
 * 「配对请求 + 领取后换新凭证」的状态迁移。
 */
export interface ClientPairingHost {
  credentials: ICredentialService;
  fetchImpl?: typeof fetch;
  getServerUrl(): string | null;
  ensureProfile(serverUrl: string): Promise<RemoteV3ClientProfile>;
  refreshClient(
    serverUrl: string,
    profile: RemoteV3ClientProfile,
  ): Promise<RemoteV3ClientProfile>;
  applyProfile(profile: RemoteV3ClientProfile): void;
  dispose(): void;
}

export function createClientPairingMethods(host: ClientPairingHost) {
  const context = async (): Promise<{ serverUrl: string; profile: RemoteV3ClientProfile }> => {
    const serverUrl = host.getServerUrl();
    if (!serverUrl) throw new Error("Host 尚未登录远程控制 v3");
    const profile = await host.ensureProfile(serverUrl);
    if (!profile.accessToken) throw new Error("Client 凭证缺失");
    return { serverUrl, profile };
  };

  const withToken = async <T>(
    serverUrl: string,
    profile: RemoteV3ClientProfile,
    run: (accessToken: string) => Promise<T>,
  ): Promise<T> => {
    try {
      return await run(profile.accessToken!);
    } catch (error) {
      if (!isRemoteV3AuthError(error)) throw error;
      const refreshed = await host.refreshClient(serverUrl, profile);
      return await run(refreshed.accessToken!);
    }
  };

  const fetchOption = host.fetchImpl ? { fetchImpl: host.fetchImpl } : {};

  return {
    async claimPairing(ticket: string): Promise<RemoteV3PairingClaim> {
      const { serverUrl, profile } = await context();
      return await withToken(serverUrl, profile, (accessToken) =>
        claimRemoteV3Pairing({ serverUrl, accessToken, ticket, ...fetchOption }),
      );
    },
    async getPairing(pairingId: string): Promise<RemoteV3PairingState> {
      const { serverUrl, profile } = await context();
      return await withToken(serverUrl, profile, (accessToken) =>
        getRemoteV3Pairing({ serverUrl, accessToken, pairingId, ...fetchOption }),
      );
    },
    async cancelPairing(pairingId: string): Promise<RemoteV3PairingState> {
      const { serverUrl, profile } = await context();
      return await withToken(serverUrl, profile, (accessToken) =>
        cancelRemoteV3Pairing({ serverUrl, accessToken, pairingId, ...fetchOption }),
      );
    },
    async redeemPairing(pairingId: string): Promise<RemoteV3PairingState> {
      const { serverUrl, profile } = await context();
      const tokens = await withToken(serverUrl, profile, (accessToken) =>
        redeemRemoteV3Pairing({ serverUrl, accessToken, pairingId, ...fetchOption }),
      );
      // 领取即换新凭证：旧 token 已失效，现有连接全部作废，后续按新身份重建。
      await saveRemoteV3ClientTokens(host.credentials, tokens);
      host.applyProfile({
        ...profile,
        accessToken: tokens.accessToken,
        refreshToken: tokens.refreshToken,
      });
      host.dispose();
      return await this.getPairing(pairingId);
    },
  };
}
