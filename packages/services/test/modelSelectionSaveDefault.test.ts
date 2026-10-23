import assert from "node:assert/strict";
import test from "node:test";

import type { ModelSelection, ModelSelectionFacade } from "@mode/provider";
import { createModelSelectionService } from "../src/model-provider/providerFacadeServices.js";

// defaultModelSelection 写回路径回归：桌面端显式选模后经
// IModelSelectionService.saveConfiguredDefault 落到 Personal 配置。
// source 未装配写入能力时方法必须缺省（undefined），让旧 RPC 对端/调用方能容错。

const selection: ModelSelection = {
  providerId: "deepseek",
  modelId: "deepseek-flash",
  options: { reasoningLevel: "max" },
} as const;

function facadeStub(): ModelSelectionFacade {
  return {
    getView: () => ({ revision: 1, providers: [] }),
    onDidChange: () => () => {},
  } as unknown as ModelSelectionFacade;
}

test("source 装配 saveConfiguredDefault 时服务暴露同名方法并透传", async () => {
  const saved: ModelSelection[] = [];
  const service = createModelSelectionService(facadeStub(), async () => {}, {
    read: async () => undefined,
    saveConfiguredDefault: async (next) => {
      saved.push(next);
      return next;
    },
  });
  assert.equal(typeof service.saveConfiguredDefault, "function");
  await service.saveConfiguredDefault?.(selection);
  assert.equal(saved.length, 1);
  assert.equal(saved[0]?.modelId, "deepseek-flash");
  service.dispose();
});

test("source 未装配写入能力时 saveConfiguredDefault 缺省", () => {
  const service = createModelSelectionService(facadeStub(), async () => {}, {
    read: async () => undefined,
  });
  assert.equal(service.saveConfiguredDefault, undefined);
  service.dispose();
});

test("无 configuredDefaultSource 时 saveConfiguredDefault 缺省", () => {
  const service = createModelSelectionService(facadeStub());
  assert.equal(service.saveConfiguredDefault, undefined);
  service.dispose();
});

test("dispose 后调用 saveConfiguredDefault 抛错", async () => {
  const service = createModelSelectionService(facadeStub(), async () => {}, {
    read: async () => undefined,
    saveConfiguredDefault: async (next) => next,
  });
  service.dispose();
  await assert.rejects(
    () => service.saveConfiguredDefault?.(selection),
    /已 dispose/,
  );
});
