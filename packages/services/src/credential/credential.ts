import { ServiceChannels } from "@mode/shared";
import { createServiceDescriptor } from "../descriptors.js";

/**
 * 凭据管理服务
 *
 * 提供 key-value 形式的凭据读写。
 * 实现端（host process）负责加密存储细节，
 * 消费端（renderer）只通过 RPC 调用，不感知存储位置。
 */
export interface ICredentialService {
  load(key: string): Promise<string | null>;
  save(key: string, value: string): Promise<void>;
  delete(key: string): Promise<void>;
  /**
   * 按键名前缀枚举（只返回键名，不触碰值）。
   * 供「已配对设备」这类按前缀扫描的场景使用；不解密值，旧密钥条目也不会在此抛错。
   */
  list(prefix: string): Promise<string[]>;
}

export const ICredentialService = createServiceDescriptor<ICredentialService>(
  ServiceChannels.Credential,
);
