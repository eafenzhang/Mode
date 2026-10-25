import assert from "node:assert/strict";
import test from "node:test";

import type { ModelSelectionView } from "@mode/services";
import {
  buildInitialPromptComposerDraft,
  initializeComposerDraftFromSeed,
  reconcileMissingDraftModelSelection,
  resolveComposerDraftScopeId,
} from "../src/v4/composer/composerDraftInitialization.js";
import type { V4ComposerDraft } from "../src/v4/composer/composerDraftStore.js";

const userSelection = {
  providerId: "opencode-go-chat",
  modelId: "deepseek-v4.1-flash",
  options: { reasoningLevel: "max" },
} as const;

const sessionSelection = {
  providerId: "deepseek",
  modelId: "deepseek-flash",
  options: { reasoningLevel: "max" },
} as const;

function modelView(preferredSelection = sessionSelection): ModelSelectionView {
  return {
    revision: 1,
    preferredSelection,
    providers: [
      {
        providerId: "deepseek",
        providerName: "MyDeepSeek",
        config: {
          optionSpecs: { reasoningLevel: { values: ["low", "max"] } },
        },
        models: [
          {
            modelId: "deepseek-flash",
            config: { optionSpecs: { reasoningLevel: { values: ["low", "max"] } } },
          },
        ],
      },
    ],
  } as unknown as ModelSelectionView;
}

function draft(overrides: Partial<V4ComposerDraft> = {}): V4ComposerDraft {
  return { text: "", updatedAt: 1, ...overrides };
}

test("Snapshot 初始化不能覆盖 Snapshot 到达前用户已选中的模型", () => {
  const initialized = initializeComposerDraftFromSeed({
    draft: draft({ modelSelection: userSelection }),
    initializeAsNewTask: false,
    modelSelectionView: null,
    sessionConfig: {
      mode: "yolo",
      planEnabled: false,
      modelSelection: sessionSelection,
    },
    workspacePath: "D:/repo",
  });

  assert.ok(initialized);
  assert.equal(initialized.mode, "yolo");
  assert.deepEqual(initialized.modelSelection, userSelection);
});

test("Snapshot 没有模型时也不能把已选模型清空", () => {
  const initialized = initializeComposerDraftFromSeed({
    draft: draft({ modelSelection: userSelection }),
    initializeAsNewTask: false,
    modelSelectionView: null,
    sessionConfig: { mode: "yolo", planEnabled: false },
    workspacePath: "D:/repo",
  });

  assert.ok(initialized);
  assert.deepEqual(initialized.modelSelection, userSelection);
});

test("草稿没有选择时才使用 Snapshot 的模型", () => {
  const initialized = initializeComposerDraftFromSeed({
    draft: draft(),
    initializeAsNewTask: false,
    modelSelectionView: null,
    sessionConfig: {
      mode: "build",
      planEnabled: false,
      modelSelection: sessionSelection,
    },
    workspacePath: "D:/repo",
  });

  assert.ok(initialized);
  assert.equal(initialized.mode, "build");
  assert.deepEqual(initialized.modelSelection, sessionSelection);
});

test("已有 mode 的草稿不再响应后续 Snapshot", () => {
  const initialized = initializeComposerDraftFromSeed({
    draft: draft({ mode: "yolo", modelSelection: userSelection }),
    initializeAsNewTask: false,
    modelSelectionView: null,
    sessionConfig: {
      mode: "build",
      planEnabled: false,
      modelSelection: sessionSelection,
    },
    workspacePath: "D:/repo",
  });

  assert.equal(initialized, null);
});

test("新任务初始化同样保留用户已选中的模型", () => {
  const initialized = initializeComposerDraftFromSeed({
    draft: draft({ modelSelection: userSelection }),
    initializeAsNewTask: true,
    modelSelectionView: modelView(),
    sessionConfig: null,
    workspacePath: "D:/repo",
  });

  assert.ok(initialized);
  assert.equal(initialized.mode, "build");
  assert.deepEqual(initialized.modelSelection, userSelection);
});

test("草稿 scope 优先使用真实会话，其次面板 scope，最后才是 __draft__", () => {
  assert.equal(resolveComposerDraftScopeId("sess_1", "mode-agent-chat:tab"), "sess_1");
  assert.equal(resolveComposerDraftScopeId(null, "mode-agent-chat:tab"), "mode-agent-chat:tab");
  assert.equal(resolveComposerDraftScopeId(null, "  "), "__draft__");
  assert.equal(resolveComposerDraftScopeId(null, undefined), "__draft__");
});

test("带初始 Prompt 新建任务只替换正文并保留配置意图", () => {
  const next = buildInitialPromptComposerDraft(
    draft({
      text: "旧正文",
      editorStateJson: '{"root":{}}',
      mode: "yolo",
      planEnabled: true,
      modelSelection: userSelection,
      lastPermissionGrantId: "grant-1",
    }),
    "新正文",
  );

  assert.equal(next.text, "新正文");
  assert.equal(next.editorStateJson, undefined);
  assert.equal(next.mode, "yolo");
  assert.equal(next.planEnabled, true);
  assert.deepEqual(next.modelSelection, userSelection);
  assert.equal(next.lastPermissionGrantId, "grant-1");
});

test("没有旧草稿时带初始 Prompt 新建任务仍可正常写入", () => {
  const next = buildInitialPromptComposerDraft(null, "新正文");
  assert.equal(next.text, "新正文");
  assert.equal(next.mode, undefined);
  assert.equal(next.modelSelection, undefined);
});

// ── 会话真值回填（spec 规则 8，2026-10-25 重启丢模型修复）──

test("草稿缺选择且 Snapshot 带选择：回填会话真值", () => {
  const draft: V4ComposerDraft = { text: "", mode: "build", updatedAt: 1 };
  const next = reconcileMissingDraftModelSelection(draft, {
    modelSelection: { ...sessionSelection },
  });
  assert.deepEqual(next?.modelSelection, sessionSelection);
});

test("草稿已有显式选择（含与快照不同）：不覆盖（规则 2）", () => {
  const draft: V4ComposerDraft = {
    text: "",
    mode: "build",
    modelSelection: { ...userSelection },
    updatedAt: 1,
  };
  assert.equal(
    reconcileMissingDraftModelSelection(draft, {
      modelSelection: { ...sessionSelection },
    }),
    null,
  );
});

test("Snapshot 没有选择：不回填（保持空态语义）", () => {
  const draft: V4ComposerDraft = { text: "", mode: "build", updatedAt: 1 };
  assert.equal(reconcileMissingDraftModelSelection(draft, {}), null);
  assert.equal(reconcileMissingDraftModelSelection(draft, null), null);
});

test("回填结果保留草稿其余字段", () => {
  const draft: V4ComposerDraft = {
    text: "hello",
    mode: "build",
    planEnabled: true,
    lastPermissionGrantId: "perm-1",
    updatedAt: 9,
  };
  const next = reconcileMissingDraftModelSelection(draft, {
    modelSelection: { ...sessionSelection },
  });
  assert.equal(next?.text, "hello");
  assert.equal(next?.planEnabled, true);
  assert.equal(next?.lastPermissionGrantId, "perm-1");
  assert.deepEqual(next?.modelSelection, sessionSelection);
});
