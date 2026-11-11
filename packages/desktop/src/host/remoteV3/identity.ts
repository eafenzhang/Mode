import { randomUUID } from "node:crypto";

import {
  exportIdentityPrivateKey,
  generateIdentity,
  identityFingerprint,
  importIdentityFromPkcs8,
  type IdentityKeyPair,
} from "@mode/remote-core";
import type { ICredentialService } from "@mode/services";

export const REMOTE_V3_CREDENTIAL_KEYS = {
  serverUrl: "remote:v3:server-url",
  deviceId: "remote:v3:device-id",
  identity: "remote:v3:identity-pkcs8",
  accessToken: "remote:v3:access-token",
  refreshToken: "remote:v3:refresh-token",
  deviceName: "remote:v3:device-name",
  accountId: "remote:v3:account-id",
  accountName: "remote:v3:account-name",
  clientDeviceId: "remote:v3:client-device-id",
  clientIdentity: "remote:v3:client-identity-pkcs8",
  clientAccessToken: "remote:v3:client-access-token",
  clientRefreshToken: "remote:v3:client-refresh-token",
  pinPrefix: "remote:v3:pin:",
} as const;

export interface RemoteV3Tokens {
  accessToken: string;
  accessExpiresAt: number;
  refreshToken: string;
  refreshExpiresAt: number;
}

export interface RemoteV3StoredConfig {
  serverUrl: string | null;
  /** 当前设备凭证所属账号；用于切换账号时轮换设备身份。 */
  accountId: string | null;
  accountName: string | null;
  deviceId: string;
  deviceName: string | null;
  identity: IdentityKeyPair;
  fingerprint: string;
  tokens: RemoteV3Tokens | null;
}

export async function loadOrCreateRemoteV3Config(
  credentials: ICredentialService,
): Promise<RemoteV3StoredConfig> {
  const [
    storedServer,
    storedDeviceId,
    storedIdentity,
    storedAccess,
    storedRefresh,
    storedName,
    storedAccountId,
    storedAccountName,
  ] = await Promise.all([
    credentials.load(REMOTE_V3_CREDENTIAL_KEYS.serverUrl),
    credentials.load(REMOTE_V3_CREDENTIAL_KEYS.deviceId),
    credentials.load(REMOTE_V3_CREDENTIAL_KEYS.identity),
    credentials.load(REMOTE_V3_CREDENTIAL_KEYS.accessToken),
    credentials.load(REMOTE_V3_CREDENTIAL_KEYS.refreshToken),
    credentials.load(REMOTE_V3_CREDENTIAL_KEYS.deviceName),
    credentials.load(REMOTE_V3_CREDENTIAL_KEYS.accountId),
    credentials.load(REMOTE_V3_CREDENTIAL_KEYS.accountName),
  ]);

  let deviceId = storedDeviceId?.trim() ?? "";
  if (!deviceId) {
    deviceId = randomUUID();
    await credentials.save(REMOTE_V3_CREDENTIAL_KEYS.deviceId, deviceId);
  }

  let identity: IdentityKeyPair;
  if (storedIdentity) {
    identity = importIdentityFromPkcs8(storedIdentity);
  } else {
    identity = generateIdentity();
    await credentials.save(
      REMOTE_V3_CREDENTIAL_KEYS.identity,
      exportIdentityPrivateKey(identity),
    );
  }

  const tokens =
    storedAccess && storedRefresh
      ? {
          accessToken: storedAccess,
          accessExpiresAt: 0,
          refreshToken: storedRefresh,
          refreshExpiresAt: 0,
        }
      : null;

  return {
    serverUrl: storedServer?.trim() || null,
    accountId: storedAccountId?.trim() || null,
    accountName: storedAccountName?.trim() || null,
    deviceId,
    deviceName: storedName?.trim() || null,
    identity,
    fingerprint: identityFingerprint(identity.identityKey),
    tokens,
  };
}

export interface RemoteV3ClientProfile {
  deviceId: string;
  identity: IdentityKeyPair;
  fingerprint: string;
  accessToken: string | null;
  refreshToken: string | null;
}

export async function loadOrCreateRemoteV3ClientProfile(
  credentials: ICredentialService,
): Promise<RemoteV3ClientProfile> {
  const [storedDeviceId, storedIdentity, storedAccess, storedRefresh] = await Promise.all([
    credentials.load(REMOTE_V3_CREDENTIAL_KEYS.clientDeviceId),
    credentials.load(REMOTE_V3_CREDENTIAL_KEYS.clientIdentity),
    credentials.load(REMOTE_V3_CREDENTIAL_KEYS.clientAccessToken),
    credentials.load(REMOTE_V3_CREDENTIAL_KEYS.clientRefreshToken),
  ]);
  let deviceId = storedDeviceId?.trim() ?? "";
  if (!deviceId) {
    deviceId = randomUUID();
    await credentials.save(REMOTE_V3_CREDENTIAL_KEYS.clientDeviceId, deviceId);
  }
  let identity: IdentityKeyPair;
  if (storedIdentity) {
    identity = importIdentityFromPkcs8(storedIdentity);
  } else {
    identity = generateIdentity();
    await credentials.save(
      REMOTE_V3_CREDENTIAL_KEYS.clientIdentity,
      exportIdentityPrivateKey(identity),
    );
  }
  return {
    deviceId,
    identity,
    fingerprint: identityFingerprint(identity.identityKey),
    accessToken: storedAccess?.trim() || null,
    refreshToken: storedRefresh?.trim() || null,
  };
}

export async function saveRemoteV3ClientTokens(
  credentials: ICredentialService,
  tokens: { accessToken: string; refreshToken: string },
): Promise<void> {
  await Promise.all([
    credentials.save(REMOTE_V3_CREDENTIAL_KEYS.clientAccessToken, tokens.accessToken),
    credentials.save(REMOTE_V3_CREDENTIAL_KEYS.clientRefreshToken, tokens.refreshToken),
  ]);
}

export async function clearRemoteV3ClientTokens(
  credentials: ICredentialService,
): Promise<void> {
  await Promise.all([
    credentials.delete(REMOTE_V3_CREDENTIAL_KEYS.clientAccessToken),
    credentials.delete(REMOTE_V3_CREDENTIAL_KEYS.clientRefreshToken),
  ]);
}

export async function clearRemoteV3AccountName(
  credentials: ICredentialService,
): Promise<void> {
  await credentials.delete(REMOTE_V3_CREDENTIAL_KEYS.accountName);
}

export async function saveRemoteV3Account(
  credentials: ICredentialService,
  accountId: string,
  accountName: string | null,
): Promise<void> {
  await credentials.save(REMOTE_V3_CREDENTIAL_KEYS.accountId, accountId);
  if (accountName) {
    await credentials.save(REMOTE_V3_CREDENTIAL_KEYS.accountName, accountName);
  } else {
    await credentials.delete(REMOTE_V3_CREDENTIAL_KEYS.accountName);
  }
}

/**
 * 切换账号时轮换设备身份：deviceId 在服务端绑定唯一账号，
 * 沿用旧 deviceId 会被拒绝（device_conflict）。轮换后旧账号里的
 * 设备记录与 peer pin 自然失效，符合「设备只属于一个账号」的模型。
 */
export async function rotateRemoteV3HostIdentity(
  credentials: ICredentialService,
): Promise<void> {
  const identity = generateIdentity();
  const pinnedPeers = await credentials.list(REMOTE_V3_CREDENTIAL_KEYS.pinPrefix);
  await Promise.all([
    credentials.save(REMOTE_V3_CREDENTIAL_KEYS.deviceId, randomUUID()),
    credentials.save(
      REMOTE_V3_CREDENTIAL_KEYS.identity,
      exportIdentityPrivateKey(identity),
    ),
    credentials.delete(REMOTE_V3_CREDENTIAL_KEYS.accessToken),
    credentials.delete(REMOTE_V3_CREDENTIAL_KEYS.refreshToken),
    credentials.delete(REMOTE_V3_CREDENTIAL_KEYS.accountId),
    credentials.delete(REMOTE_V3_CREDENTIAL_KEYS.accountName),
    // 旧账号的 peer pin 对新账号没有意义，留着只会挡住同名 deviceId。
    ...pinnedPeers.map((key) => credentials.delete(key)),
  ]);
}

/** Client 角色同样按账号隔离：换账号时必须换身份，否则会命中旧账号的设备记录。 */
export async function rotateRemoteV3ClientIdentity(
  credentials: ICredentialService,
): Promise<void> {
  const identity = generateIdentity();
  await Promise.all([
    credentials.save(REMOTE_V3_CREDENTIAL_KEYS.clientDeviceId, randomUUID()),
    credentials.save(
      REMOTE_V3_CREDENTIAL_KEYS.clientIdentity,
      exportIdentityPrivateKey(identity),
    ),
    clearRemoteV3ClientTokens(credentials),
  ]);
}

export async function saveRemoteV3ServerUrl(
  credentials: ICredentialService,
  serverUrl: string,
): Promise<void> {
  await credentials.save(REMOTE_V3_CREDENTIAL_KEYS.serverUrl, serverUrl);
}

export async function saveRemoteV3Server(
  credentials: ICredentialService,
  serverUrl: string,
  deviceName: string,
  tokens: RemoteV3Tokens,
): Promise<void> {
  await Promise.all([
    credentials.save(REMOTE_V3_CREDENTIAL_KEYS.serverUrl, serverUrl),
    credentials.save(REMOTE_V3_CREDENTIAL_KEYS.deviceName, deviceName),
    saveRemoteV3Tokens(credentials, tokens),
  ]);
}

export async function saveRemoteV3Tokens(
  credentials: ICredentialService,
  tokens: RemoteV3Tokens,
): Promise<void> {
  await Promise.all([
    credentials.save(REMOTE_V3_CREDENTIAL_KEYS.accessToken, tokens.accessToken),
    credentials.save(REMOTE_V3_CREDENTIAL_KEYS.refreshToken, tokens.refreshToken),
  ]);
}

export async function clearRemoteV3Tokens(credentials: ICredentialService): Promise<void> {
  await Promise.all([
    credentials.delete(REMOTE_V3_CREDENTIAL_KEYS.accessToken),
    credentials.delete(REMOTE_V3_CREDENTIAL_KEYS.refreshToken),
  ]);
}

/** 首次见面固定公钥；已知 deviceId 换钥返回 false，必须显式重置 pin 才能重连。 */
export async function verifyOrPinPeer(
  credentials: ICredentialService,
  peer: { deviceId: string; identityKey: string },
): Promise<boolean> {
  const key = `${REMOTE_V3_CREDENTIAL_KEYS.pinPrefix}${peer.deviceId}`;
  const pinned = await credentials.load(key);
  if (!pinned) {
    await credentials.save(key, peer.identityKey);
    return true;
  }
  return pinned === peer.identityKey;
}

export async function resetRemoteV3PeerPin(
  credentials: ICredentialService,
  deviceId: string,
): Promise<void> {
  await credentials.delete(`${REMOTE_V3_CREDENTIAL_KEYS.pinPrefix}${deviceId}`);
}
