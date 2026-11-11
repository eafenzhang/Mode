/** 浏览器账号会话的类型（account/accountSupport 共用，避免运行时循环依赖）。 */

export interface WebCredentialStore {
  get(key: string): Promise<string | null>;
  set(key: string, value: string): Promise<void>;
  delete(key: string): Promise<void>;
}

export interface WebDeviceProfile {
  serverUrl: string;
  deviceId: string;
  deviceName: string;
  /** X25519 私钥，原始 32 字节 base64url；不写日志/URL。 */
  privateKey: string;
  accessToken: string | null;
  refreshToken: string | null;
}

export interface WebDeviceSummary {
  deviceId: string;
  name: string;
  role: "host" | "client";
  platform: string;
  /** 设备公钥（目录里公开的身份标识；用于展示指纹/设备列表去重）。 */
  identityKey: string;
  online: boolean;
  addresses: string[];
}
