import assert from "node:assert/strict";
import test from "node:test";

import {
  clampIndex,
  filterSlashCommands,
  modelCommandQuery,
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
