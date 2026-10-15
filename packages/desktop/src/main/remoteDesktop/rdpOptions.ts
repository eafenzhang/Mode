/**
 * 主控端「每设备 RDP 连接选项」存储（docs/specs/remote-desktop-system-rdp.md 状态所有者表）。
 *
 * 归属主控 main：这些是**控制端自己的偏好**（窗口形态/剪贴板/音频/用户名/端口），与
 * 被控端设置无关，故不与被控共享开关同存储（那个在 host 的凭据服务里）。
 *
 * 用户名不是机密，端口也不是，故落普通 JSON 而非凭据服务；损坏/非法按空表降级。
 */
import { readFile, rename, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { z } from "zod";
import {
  rdpConnectionOptionsSchema,
  type RdpConnectionOptions,
} from "@mode/shared/remote-desktop-rdp";

const rdpOptionsFileSchema = z.record(z.string(), rdpConnectionOptionsSchema);

export interface RdpOptionsStore {
  read(serverId: string): Promise<RdpConnectionOptions>;
  write(serverId: string, options: RdpConnectionOptions): Promise<void>;
}

const OPTIONS_FILE_NAME = "rdp-options.json";

export function createRdpOptionsStore(userDataDir: string): RdpOptionsStore {
  const filePath = join(userDataDir, OPTIONS_FILE_NAME);

  const readAll = async (): Promise<Record<string, RdpConnectionOptions>> => {
    try {
      const parsed = rdpOptionsFileSchema.safeParse(JSON.parse(await readFile(filePath, "utf8")));
      return parsed.success ? parsed.data : {};
    } catch {
      // 文件不存在（首次）或损坏：都按空表；写回时整体覆盖，损坏自然被修好。
      return {};
    }
  };

  return {
    async read(serverId: string): Promise<RdpConnectionOptions> {
      const all = await readAll();
      return all[serverId] ?? {};
    },
    async write(serverId: string, options: RdpConnectionOptions): Promise<void> {
      const all = await readAll();
      const normalized = rdpConnectionOptionsSchema.safeParse(options);
      if (normalized.success && Object.keys(normalized.data).length > 0) {
        all[serverId] = normalized.data;
      } else {
        delete all[serverId];
      }
      // 先写临时文件再改名：半截文件不会覆盖掉上一份可用配置。
      const tempPath = `${filePath}.tmp`;
      await writeFile(tempPath, JSON.stringify(all, null, 2), "utf8");
      await rename(tempPath, filePath);
    },
  };
}
