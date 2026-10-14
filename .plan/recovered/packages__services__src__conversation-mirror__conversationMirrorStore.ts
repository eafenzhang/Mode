import { readFile, readdir } from "node:fs/promises";
import { join } from "node:path";
import {
  conversationMirrorSnapshotFileStem,
  conversationMirrorSnapshotSchema,
  type ConversationMirrorSnapshot,
} from "@mode/shared";
import { atomicWriteText } from "../fs/atomicFileUtils.js";
import { createServiceLogger } from "../logger/serviceLogger.js";
import { getConversationMirrorDir } from "../paths.js";

const logger = createServiceLogger("conversation-mirror");

const SNAPSHOT_FILE_EXTENSION = ".json";

/**
 * 镜像快照的磁盘存储：`<数据根>/v2/conversation-mirror/<workspacePathHash>/<originDeviceId>.json`。
 *
 * 一个 origin 一个文件是幂等的关键：对端重复发布只覆盖自己的文件，不会叠加记录，也不会
 * 影响其它 origin。读取端对单文件损坏只跳过该文件（fail-soft），不拖垮整个工作区的镜像。
 */
export interface ConversationMirrorStore {
  readWorkspaceSnapshots(workspacePath: string): Promise<ConversationMirrorSnapshot[]>;
  writeSnapshot(snapshot: ConversationMirrorSnapshot): Promise<void>;
}

function isMissingPathError(error: unknown): boolean {
  return (error as NodeJS.ErrnoException | undefined)?.code === "ENOENT";
}

export function createConversationMirrorStore(): ConversationMirrorStore {
  return {
    async readWorkspaceSnapshots(workspacePath: string): Promise<ConversationMirrorSnapshot[]> {
      const dir = getConversationMirrorDir(workspacePath);
      let entries: string[];
      try {
        entries = await readdir(dir);
      } catch (error) {
        if (!isMissingPathError(error)) {
          // 目录不可读属于环境问题（权限/盘符失效），镜像降级为空而不是让调用方失败。
          logger.warn(undefined, "读取对话镜像目录失败", { dir, error });
        }
        return [];
      }

      const snapshots: ConversationMirrorSnapshot[] = [];
      for (const entry of entries) {
        if (!entry.endsWith(SNAPSHOT_FILE_EXTENSION)) {
          continue;
        }
        const file = join(dir, entry);
        try {
          const parsed = conversationMirrorSnapshotSchema.safeParse(
            JSON.parse(await readFile(file, "utf-8")),
          );
          if (!parsed.success) {
            logger.warn(undefined, "跳过无效的对话镜像快照", { file });
            continue;
          }
          snapshots.push(parsed.data);
        } catch (error) {
          logger.warn(undefined, "跳过无法解析的对话镜像快照", { file, error });
        }
      }
      return snapshots;
    },

    async writeSnapshot(snapshot: ConversationMirrorSnapshot): Promise<void> {
      const file = join(
        getConversationMirrorDir(snapshot.workspacePath),
        `${conversationMirrorSnapshotFileStem(snapshot.originDeviceId)}${SNAPSHOT_FILE_EXTENSION}`,
      );
      await atomicWriteText(file, JSON.stringify(snapshot));
    },
  };
}
