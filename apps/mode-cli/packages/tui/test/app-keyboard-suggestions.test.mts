import assert from "node:assert/strict";
import test from "node:test";

import { handleSuggestionNavigationKey } from "../src/app-keyboard-suggestions.js";

interface Selection {
  selectedIndex: number;
}

function run(
  keyName: string,
  options: {
    models?: readonly unknown[];
    modelSelection?: Selection;
    efforts?: readonly unknown[];
    effortSelection?: Selection;
    modes?: readonly unknown[];
    modeSelection?: Selection;
    slash?: readonly unknown[];
    slashSelection?: Selection;
  } = {},
) {
  const setters: Record<string, Selection | undefined> = {};
  const consumed: string[] = [];
  const apply = (name: string, selection: Selection | undefined) => (value: unknown) => {
    setters[name] =
      typeof value === "function"
        ? (value as (previous: Selection | undefined) => Selection | undefined)(selection)
        : (value as Selection | undefined);
  };
  const result = handleSuggestionNavigationKey({
    key: { name: keyName } as never,
    filteredModelOptions: options.models ?? [],
    modelSelection: options.modelSelection,
    setModelSelection: apply("model", options.modelSelection),
    filteredEffortOptions: options.efforts ?? [],
    effortSelection: options.effortSelection,
    setEffortSelection: apply("effort", options.effortSelection),
    filteredModeOptions: options.modes ?? [],
    modeSelection: options.modeSelection,
    setModeSelection: apply("mode", options.modeSelection),
    filteredSlashCommands: options.slash ?? [],
    slashSelection: options.slashSelection,
    setSlashSelection: apply("slash", options.slashSelection),
    consumeKey: (key: { name: string }) => consumed.push(key.name),
  } as never);
  return { result, setters, consumed };
}

test("建议导航：非上下键不消费，无选中态即使有候选也不处理", () => {
  const left = run("left", { models: [{}, {}], modelSelection: { selectedIndex: 0 } });
  assert.deepEqual(left, { result: false, setters: {}, consumed: [] });
  const noSelection = run("down", { models: [{}, {}] });
  assert.deepEqual(noSelection, { result: false, setters: {}, consumed: [] });
});

test("建议导航：模型建议上下移动并在边界夹取，按键被消费", () => {
  const down = run("down", { models: [{}, {}], modelSelection: { selectedIndex: 0 } });
  assert.equal(down.result, true);
  assert.deepEqual(down.setters.model, { selectedIndex: 1 });
  assert.deepEqual(down.consumed, ["down"]);
  const up = run("up", { models: [{}, {}], modelSelection: { selectedIndex: 0 } });
  assert.deepEqual(up.setters.model, { selectedIndex: 0 });
  const last = run("down", { models: [{}, {}], modelSelection: { selectedIndex: 1 } });
  assert.deepEqual(last.setters.model, { selectedIndex: 1 });
});

test("建议导航：优先级 model → effort → mode → slash", () => {
  const modelWins = run("down", {
    models: [{}, {}],
    modelSelection: { selectedIndex: 0 },
    efforts: [{}, {}],
    effortSelection: { selectedIndex: 0 },
  });
  assert.deepEqual(modelWins.setters.model, { selectedIndex: 1 });
  assert.equal(modelWins.setters.effort, undefined);

  const effortFallback = run("down", { efforts: [{}, {}], effortSelection: { selectedIndex: 0 } });
  assert.deepEqual(effortFallback.setters.effort, { selectedIndex: 1 });

  const modeFallback = run("down", { modes: [{}, {}], modeSelection: { selectedIndex: 0 } });
  assert.deepEqual(modeFallback.setters.mode, { selectedIndex: 1 });

  const slashFallback = run("down", { slash: [{}, {}], slashSelection: { selectedIndex: 0 } });
  assert.deepEqual(slashFallback.setters.slash, { selectedIndex: 1 });
});
