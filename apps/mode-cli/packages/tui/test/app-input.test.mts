import assert from "node:assert/strict";
import test from "node:test";

import {
  clampIndex,
  filterEffortOptions,
  filterModelOptions,
  isValidClipboardImage,
  filterSlashCommands,
  modelCommandQuery,
  reconcileModelCommandSelection,
  selectedModelOption,
  reconcileEffortCommandSelection,
  selectedEffortOption,
  visibleEffortOptionWindow,
  visibleSlashCommandWindow,
} from "../src/app-input.js";

test("clampIndex：负数归 0，越界归末位，空集合归 0", () => {
  assert.equal(clampIndex(-1, 3), 0);
  assert.equal(clampIndex(0, 3), 0);
  assert.equal(clampIndex(2, 3), 2);
  assert.equal(clampIndex(5, 3), 2);
  assert.equal(clampIndex(1, 0), 0);
});

test("modelCommandQuery：/model 前缀解析，仅空格分隔的查询保留原样", () => {
  assert.equal(modelCommandQuery("/model"), "");
  assert.equal(modelCommandQuery("/model "), "");
  assert.equal(modelCommandQuery("/model gpt"), "gpt");
  assert.equal(modelCommandQuery("/model   gpt"), "  gpt");
  assert.equal(modelCommandQuery("hello"), undefined);
  assert.equal(modelCommandQuery(" /model gpt"), undefined);
  assert.equal(modelCommandQuery("/MODEL gpt"), undefined);
  assert.equal(modelCommandQuery("/model\tgpt"), undefined);
});

const SLASH_COMMANDS = [
  { name: "help", aliases: ["h"] },
  { name: "model" },
  { name: "compact" },
] as never;

test("filterSlashCommands：按名称/别名包含匹配，大小写不敏感，空白终止", () => {
  const names = (draft: string) =>
    filterSlashCommands(draft, SLASH_COMMANDS).map((command) => command.name);
  assert.deepEqual(names("/"), ["help", "model", "compact"]);
  assert.deepEqual(names("/h"), ["help"]);
  assert.deepEqual(names("/HELP"), ["help"]);
  assert.deepEqual(names("/o"), ["model", "compact"]);
  assert.deepEqual(names("/model "), []);
  assert.deepEqual(names("/x"), []);
  assert.deepEqual(names("help"), []);
});

test("visibleSlashCommandWindow：窗口跟随选中项并映射到窗口内下标", () => {
  const commands = ["a", "b", "c", "d", "e"].map((name) => ({ name })) as never;
  assert.deepEqual(visibleSlashCommandWindow(commands, 0, 2), {
    commands: [{ name: "a" }, { name: "b" }],
    selectedIndex: 0,
    startIndex: 0,
  });
  assert.deepEqual(visibleSlashCommandWindow(commands, 2, 2), {
    commands: [{ name: "b" }, { name: "c" }],
    selectedIndex: 1,
    startIndex: 1,
  });
  assert.deepEqual(visibleSlashCommandWindow(commands, 4, 2), {
    commands: [{ name: "d" }, { name: "e" }],
    selectedIndex: 1,
    startIndex: 3,
  });
  assert.deepEqual(visibleSlashCommandWindow(commands, 0, 0), {
    commands: [],
    selectedIndex: 0,
    startIndex: 0,
  });
});

const EFFORTS = [
  { id: "low", label: "Low" },
  { id: "high", label: "High", description: "max effort" },
] as never;

test("effort 过滤：/effort 前缀、空查询全量、匹配 id/label/description", () => {
  assert.deepEqual(filterEffortOptions("/effort ", EFFORTS), EFFORTS);
  assert.deepEqual(filterEffortOptions("/effort h", EFFORTS), [EFFORTS[1]]);
  assert.deepEqual(filterEffortOptions("/effort max", EFFORTS), [EFFORTS[1]]);
  assert.deepEqual(filterEffortOptions("/effort zzz", EFFORTS), []);
  assert.deepEqual(filterEffortOptions("hello", EFFORTS), []);
});

test("effort 选择：命令态才产出选中项，下标夹取到有效范围", () => {
  assert.deepEqual(reconcileEffortCommandSelection("/effort ", EFFORTS), {
    selectedIndex: 0,
  });
  assert.equal(reconcileEffortCommandSelection("hello", EFFORTS), undefined);
  assert.equal(reconcileEffortCommandSelection("/effort ", [] as never), undefined);
  assert.deepEqual(selectedEffortOption("/effort high", { selectedIndex: 1 }, EFFORTS), EFFORTS[1]);
  assert.deepEqual(selectedEffortOption("/effort high", { selectedIndex: 9 }, EFFORTS), EFFORTS[1]);
  assert.equal(selectedEffortOption("hello", { selectedIndex: 0 }, EFFORTS), undefined);
  assert.equal(selectedEffortOption("/effort high", undefined, EFFORTS), undefined);
});

test("effort 可视窗口：越界夹取并滚动起点", () => {
  assert.deepEqual(visibleEffortOptionWindow(EFFORTS, 1, 1), {
    efforts: [EFFORTS[1]],
    selectedIndex: 0,
    startIndex: 1,
  });
  assert.deepEqual(visibleEffortOptionWindow(EFFORTS, 99, 2), {
    efforts: EFFORTS,
    selectedIndex: 1,
    startIndex: 0,
  });
  assert.deepEqual(visibleEffortOptionWindow([], 0, 8), {
    efforts: [],
    selectedIndex: 0,
    startIndex: 0,
  });
});

const MODELS = [
  { ref: { providerId: "openai", modelId: "gpt-5" }, label: "GPT-5", providerLabel: "OpenAI" },
  {
    ref: { providerId: "anthropic", modelId: "claude" },
    label: "Claude",
    providerLabel: "Anthropic",
  },
] as never;

test("model 过滤：按 provider/model 值、label、providerLabel 匹配", () => {
  assert.deepEqual(filterModelOptions("/model gpt", MODELS), [MODELS[0]]);
  assert.deepEqual(filterModelOptions("/model openai", MODELS), [MODELS[0]]);
  assert.deepEqual(filterModelOptions("/model openai/gpt", MODELS), [MODELS[0]]);
  assert.deepEqual(filterModelOptions("/model anthropic", MODELS), [MODELS[1]]);
  assert.deepEqual(filterModelOptions("/model zzz", MODELS), []);
  assert.deepEqual(filterModelOptions("hello", MODELS), []);
});

test("model 过滤：空查询返回全量", () => {
  assert.deepEqual(filterModelOptions("/model ", MODELS), MODELS);
});

test("model 选择：命令态才产出选中项，下标夹取到有效范围", () => {
  assert.deepEqual(reconcileModelCommandSelection("/model ", MODELS), { selectedIndex: 0 });
  assert.equal(reconcileModelCommandSelection("hello", MODELS), undefined);
  assert.equal(reconcileModelCommandSelection("/model ", [] as never), undefined);
  assert.deepEqual(selectedModelOption("/model gpt", { selectedIndex: 0 }, MODELS), MODELS[0]);
  assert.deepEqual(selectedModelOption("/model gpt", { selectedIndex: 9 }, MODELS), MODELS[1]);
  assert.equal(selectedModelOption("hello", { selectedIndex: 0 }, MODELS), undefined);
  assert.equal(selectedModelOption("/model gpt", undefined, MODELS), undefined);
});

test("剪贴板图片：仅接受受支持的 image/* 且带 data URL 的输入", () => {
  assert.equal(
    isValidClipboardImage({
      mediaType: "image/png",
      dataUrl: "data:image/png;base64,abc",
    } as never),
    true,
  );
  assert.equal(
    isValidClipboardImage({
      mediaType: "image/jpeg",
      dataUrl: "data:image/jpeg;base64,abc",
    } as never),
    true,
  );
  assert.equal(isValidClipboardImage({ mediaType: "text/plain", dataUrl: "x" } as never), false);
  assert.equal(isValidClipboardImage({ mediaType: "image/png", dataUrl: "x" } as never), false);
  assert.equal(isValidClipboardImage({ mediaType: "image/png", dataUrl: "" } as never), false);
  assert.equal(
    isValidClipboardImage({ mediaType: "", dataUrl: "data:image/png;base64,a" } as never),
    false,
  );
});
