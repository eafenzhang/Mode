import type { WebAccountClient, WebDeviceProfile } from "@mode/remote-core/web";
import type { RemoteV3PairingClaim, RemoteV3PairingState } from "@mode/shared";

export interface WebPairingHost {
  requireProfile: () => Promise<WebDeviceProfile>;
  accountFor: (profile: WebDeviceProfile) => WebAccountClient;
  /** redeem 后重新读取落盘 profile（WebAccountClient 已换新 token）。 */
  reloadProfile: () => Promise<WebDeviceProfile | null>;
  applyProfile: (profile: WebDeviceProfile) => void;
}

/**
 * 浏览器 Client 的配对方法（Host 确认面在桌面端）。
 *
 * redeem 成功后本机凭证已轮换：刷新服务侧 profile 快照，避免后续 Control
 * 重连继续使用旧 access token。
 */
export function createWebPairingMethods(host: WebPairingHost) {
  return {
    async claimPairing(ticket: string): Promise<RemoteV3PairingClaim> {
      const current = await host.requireProfile();
      return await host.accountFor(current).claimPairing(ticket);
    },
    async getPairing(pairingId: string): Promise<RemoteV3PairingState> {
      const current = await host.requireProfile();
      return await host.accountFor(current).getPairing(pairingId);
    },
    async redeemPairing(pairingId: string): Promise<RemoteV3PairingState> {
      const current = await host.requireProfile();
      const account = host.accountFor(current);
      await account.redeemPairing(pairingId);
      const next = await host.reloadProfile();
      if (next) host.applyProfile(next);
      return await account.getPairing(pairingId);
    },
    async cancelPairing(pairingId: string): Promise<RemoteV3PairingState> {
      const current = await host.requireProfile();
      const account = host.accountFor(current);
      await account.cancelPairing(pairingId);
      return await account.getPairing(pairingId);
    },
  };
}
