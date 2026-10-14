import { hostname } from "node:os";
import {
  boundConversationMirrorSnapshot,
  conversationMirrorSnapshotSchema,
  type ConversationMirrorIdentity,
  type ConversationMirrorPublishResult,
} from "@mode/shared";
import { ensureDeviceMid } from "../device/deviceMid.js";
import { createServiceLogger } from "../logger/serviceLogger.js";
import type { IConversationMirrorService } from "./conversationMirror.js";
import {
  createConversationMirrorStore,
  type ConversationMirrorStore,
} from "./conversationMirrorStore.js";

const logger = createServiceLogger("conversation-mirror");

const MAX_ORIGIN_LABEL_LENGTH = 64;
const FALLBACK_DEVICE_LABEL = "Mode";

/** 展示名只用主机名：镜像条目要让人看出记录来自哪台机器，但不暴露路径/账号。 */
function resolveDeviceLabel(): string {
  const name = hostname().trim();
  if (!name) {
    return FALLBACK_DEVICE_LABEL;
  }
  return name.slice(0, MAX_ORIGIN_LABEL_LENGTH);
}

export function createConversationMirrorService(options?: {
  store?: ConversationMirrorStore;
}): IConversationMirrorService {
  const store = options?.store ?? createConversationMirrorStore();
  let identityPromise: Promise<ConversationMirrorIdentity> | null = null;

  function resolveIdentity(): Promise<ConversationMirrorIdentity> {
    // 设备身份文件可能正被另一个进程加锁创建；失败不缓存，下一次调用重试。
    identityPromise ??= ensureDeviceMid()
      .then((deviceId) => ({ deviceId, label: resolveDeviceLabel() }))
      .catch((error: unknown) => {
        identityPromise = null;
        throw error;
      });
    return identityPromise;
  }

  /**
   * 两条入口共用同一落盘逻辑，差别只在 origin 归属校验：
   * `expectSelf` 为 true 表示本机自快照通道（必须是自己），false 表示对端发布通道（必须是别人）。
   */
  async function writeWithOriginCheck(
    snapshot: unknown,
    expectSelf: boolean,
  ): Promise<ConversationMirrorPublishResult> {
    // 对端是另一个进程：payload 必须重新校验，不能假设它已经过 schema。
    const parsed = conversationMirrorSnapshotSchema.safeParse(snapshot);
    if (!parsed.success) {
      logger.warn(undefined, "拒绝无效的对话镜像快照");
      return { stored: false, reason: "invalid-record", recordCount: 0 };
    }

    const identity = await resolveIdentity();
    const isSelf = parsed.data.originDeviceId === identity.deviceId;
    if (expectSelf !== isSelf) {
      // 对端转发本机记录（回声）或对端冒充本机记录，都不得写进本机镜像目录。
      return { stored: false, reason: expectSelf ? "foreign-origin" : "self-origin", recordCount: 0 };
    }

    const bounded = boundConversationMirrorSnapshot(parsed.data);
    await store.writeSnapshot(bounded);
    return { stored: true, reason: "stored", recordCount: bounded.records.length };
  }

  return {
    getMirrorIdentity(): Promise<ConversationMirrorIdentity> {
      return resolveIdentity();
    },

    publishSnapshot({ snapshot }): Promise<ConversationMirrorPublishResult> {
      return writeWithOriginCheck(snapshot, false);
    },

    writeLocalSnapshot({ snapshot }): Promise<ConversationMirrorPublishResult> {
      return writeWithOriginCheck(snapshot, true);
    },

    async readWorkspaceSnapshots({ workspacePath }) {
      return { snapshots: await store.readWorkspaceSnapshots(workspacePath) };
    },
  };
}
