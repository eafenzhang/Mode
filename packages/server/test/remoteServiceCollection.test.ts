/**
 * `/ws/remote/:id` 的完整 service 注册（docs/specs/web-remote-workspace.md）。
 *
 * 远端 accessor 的 local-only 可选服务可能不存在；这里验证必选服务全部注册、
 * 可选缺失时不被伪造为空实例。
 */
import assert from "node:assert/strict";
import test from "node:test";

import {
  IFileService,
  ILanAccessService,
  IModeAgentService,
  ISettingService,
  type IServiceAccessor,
} from "@mode/services";

import { createRemoteServiceCollection } from "../src/remote/remoteServiceCollection.js";

const REQUIRED_STUB_KEYS = [
  "fileService",
  "gitService",
  "gitCheckpointService",
  "systemService",
  "terminalService",
  "settingService",
  "credentialService",
  "broadcastService",
  "modeTaskService",
  "modeAgentService",
  "modeSessionService",
  "conversationShareService",
  "botsService",
  "fileWatcherService",
  "oauthService",
  "providerSettingsService",
  "modelSelectionService",
  "usageStatsService",
  "codingPlanSubscriptionService",
  "clientConfigService",
  "clientScenesService",
  "offPeakTaskService",
  "skillsService",
  "skillSyncService",
  "mcpSyncService",
  "pluginSyncService",
  "pluginsService",
  "pluginManagementService",
  "subagentsService",
  "commandsService",
  "hooksService",
  "memoryService",
  "settingsSyncService",
  "promptAttachmentTransferService",
] as const;

function createAccessor(): IServiceAccessor {
  const accessor: Record<string, unknown> = {};
  for (const key of REQUIRED_STUB_KEYS) {
    accessor[key] = { [key]: true };
  }
  return accessor as unknown as IServiceAccessor;
}

test("必选远端服务全部注册，WS 客户端能拿到完整 accessor", () => {
  const accessor = createAccessor();
  const collection = createRemoteServiceCollection(accessor);
  assert.equal(collection.get(IFileService), accessor.fileService);
  assert.equal(collection.get(ISettingService), accessor.settingService);
  assert.equal(collection.get(IModeAgentService), accessor.modeAgentService);
});

test("可选 local-only 服务缺失时不注册，不伪造空实例", () => {
  const accessor = createAccessor();
  const collection = createRemoteServiceCollection(accessor);
  assert.equal(collection.getOptional(ILanAccessService), undefined);
});
