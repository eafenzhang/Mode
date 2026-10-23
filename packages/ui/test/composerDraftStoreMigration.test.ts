import assert from "node:assert/strict";
import test from "node:test";

import {
  persistV4ComposerDraft,
  readV4ComposerDraft,
} from "../src/v4/composer/composerDraftStore.js";
import { readComposerRecent } from "../src/lib/composerRecent.js";

// P2 改名（zcode- → mode-）没有迁移旧键：升级后旧键里的草稿/最近选择读不到，
// 重启表现为丢模型/丢正文。回归两处读取侧的一次性搬迁语义。

interface FakeStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

function createFakeStorage(initial: Record<string, string> = {}): FakeStorage {
  const map = new Map(Object.entries(initial));
  return {
    getItem: (key) => map.get(key) ?? null,
    setItem: (key, value) => {
      map.set(key, value);
    },
    removeItem: (key) => {
      map.delete(key);
    },
  };
}

// composerDraftStore 无 storage 注入点，走 window.localStorage；测试里装假 window。
function withFakeWindow(storage: FakeStorage, run: () => void): void {
  const previous = (globalThis as { window?: unknown }).window;
  (globalThis as { window?: unknown }).window = { localStorage: storage };
  try {
    run();
  } finally {
    if (previous === undefined) {
      delete (globalThis as { window?: unknown }).window;
    } else {
      (globalThis as { window?: unknown }).window = previous;
    }
  }
}

const workspacePath = "D:\\ZCodium";
const workspaceKey = encodeURIComponent(workspacePath);
const legacyDraftKey = `zcode-v4-composer-drafts:v1:${workspaceKey}`;
const modeDraftKey = `mode-v4-composer-drafts:v1:${workspaceKey}`;

const draftFile = {
  version: 1,
  scopes: {
    __draft__: {
      text: "hello",
      mode: "build",
      modelSelection: {
        providerId: "deepseek",
        modelId: "deepseek-flash",
        options: { reasoningLevel: "max" },
      },
      updatedAt: 1760000000000,
    },
  },
};

test("旧 zcode- 草稿键在新键为空时搬迁到 mode- 并删除旧键", () => {
  const storage = createFakeStorage({ [legacyDraftKey]: JSON.stringify(draftFile) });
  withFakeWindow(storage, () => {
    const draft = readV4ComposerDraft(workspacePath, undefined, "__draft__");
    assert.equal(draft?.text, "hello");
    assert.equal(draft?.modelSelection?.providerId, "deepseek");
    // 旧键已删除，新键已有搬迁内容
    assert.equal(storage.getItem(legacyDraftKey), null);
    assert.ok(storage.getItem(modeDraftKey));
  });
});

test("新键已有数据时不读旧键、不搬迁", () => {
  const currentFile = { version: 1, scopes: { __draft__: { text: "new", updatedAt: 1 } } };
  const storage = createFakeStorage({
    [modeDraftKey]: JSON.stringify(currentFile),
    [legacyDraftKey]: JSON.stringify(draftFile),
  });
  withFakeWindow(storage, () => {
    const draft = readV4ComposerDraft(workspacePath, undefined, "__draft__");
    assert.equal(draft?.text, "new");
    // 旧键原样保留，不做任何合并
    assert.ok(storage.getItem(legacyDraftKey));
  });
});

test("搬迁后 persistV4ComposerDraft 写入新键", () => {
  const storage = createFakeStorage({ [legacyDraftKey]: JSON.stringify(draftFile) });
  withFakeWindow(storage, () => {
    persistV4ComposerDraft(workspacePath, undefined, "sess_1", {
      text: "second",
      mode: "build",
    });
    assert.equal(storage.getItem(legacyDraftKey), null);
    const migrated = JSON.parse(storage.getItem(modeDraftKey) ?? "{}") as {
      scopes: Record<string, { text: string }>;
    };
    // 搬迁的 __draft__ 与新写的 sess_1 共存
    assert.equal(migrated.scopes.__draft__.text, "hello");
    assert.equal(migrated.scopes.sess_1.text, "second");
  });
});

test("旧键数据损坏时搬迁是原样搬运，读取仍返回空且不抛错", () => {
  const storage = createFakeStorage({ [legacyDraftKey]: "{not-json" });
  withFakeWindow(storage, () => {
    const draft = readV4ComposerDraft(workspacePath, undefined, "__draft__");
    assert.equal(draft, null);
    // 搬迁只搬原始字节，不做 JSON 校验；损坏数据搬到新键后解析失败返回空，
    // 与损坏数据留在旧键的读取结果一致，无行为回退。
    assert.equal(storage.getItem(legacyDraftKey), null);
  });
});

test("composerRecent 旧 zcode- 键搬迁到 mode- 并删除旧键", () => {
  const recentRecord = {
    modelSelection: {
      providerId: "deepseek",
      modelId: "deepseek-flash",
      options: { reasoningLevel: "max" },
    },
    mode: "build",
  };
  const storage = createFakeStorage({
    [`zcode-model-selection-recent-v1:${workspacePath}`]: JSON.stringify(recentRecord),
  });
  const recent = readComposerRecent(workspacePath, undefined, storage);
  assert.equal(recent?.modelSelection?.modelId, "deepseek-flash");
  assert.equal(storage.getItem(`zcode-model-selection-recent-v1:${workspacePath}`), null);
  assert.ok(storage.getItem(`mode-model-selection-recent-v1:${workspacePath}`));
});

test("composerRecent 新键已有数据时不读旧键", () => {
  const storage = createFakeStorage({
    [`mode-model-selection-recent-v1:${workspacePath}`]: JSON.stringify({
      modelSelection: { providerId: "xiaomi-mimo", modelId: "mimo-v2.6-flash" },
      mode: "build",
    }),
    [`zcode-model-selection-recent-v1:${workspacePath}`]: JSON.stringify({
      modelSelection: { providerId: "deepseek", modelId: "deepseek-flash" },
      mode: "build",
    }),
  });
  const recent = readComposerRecent(workspacePath, undefined, storage);
  assert.equal(recent?.modelSelection?.providerId, "xiaomi-mimo");
});
