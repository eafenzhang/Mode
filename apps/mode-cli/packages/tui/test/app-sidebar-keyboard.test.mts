import assert from "node:assert/strict";
import test from "node:test";

import { handleSidebarShortcutKey } from "../src/app-sidebar-keyboard.js";
import { createSidebarShortcutState } from "../src/app-sidebar-shortcut.js";

interface RunOptions {
  apisExpanded?: boolean;
  filesExpanded?: boolean;
  sidebarShown?: boolean;
}

function run(state: unknown, key: { ctrl?: boolean; name: string }, options: RunOptions = {}) {
  const calls: {
    consumed?: boolean;
    section?: string;
    status?: string;
    toggleSidebar?: boolean;
  } = {};
  const result = handleSidebarShortcutKey({
    consumeKey: () => {
      calls.consumed = true;
    },
    key: key as never,
    nowMs: 1000,
    setStatus: ((value: unknown) => {
      calls.status =
        typeof value === "function" ? (value as (previous: string) => string)("") : String(value);
    }) as never,
    shortcutState: state as never,
    toggleSidebar: () => {
      calls.toggleSidebar = true;
      return options.sidebarShown ?? true;
    },
    toggleSidebarSection: (section: string) => {
      calls.section = section;
      return section === "apis" ? (options.apisExpanded ?? true) : (options.filesExpanded ?? true);
    },
  } as never);
  return { result, calls };
}

test("侧栏快捷键：Ctrl+X 仅消费按键并提示 leader 选项", () => {
  assert.deepEqual(run(createSidebarShortcutState(), { name: "x", ctrl: true }), {
    result: true,
    calls: {
      consumed: true,
      status: "Press B for sidebar, M for files, A for APIs.",
    },
  });
});

test("侧栏快捷键：leader 后 B 切换侧栏并按可见性给状态", () => {
  const state = createSidebarShortcutState();
  run(state, { name: "x", ctrl: true });
  assert.deepEqual(run(state, { name: "b" }), {
    result: true,
    calls: { consumed: true, toggleSidebar: true, status: "Sidebar shown." },
  });

  const hidden = createSidebarShortcutState();
  run(hidden, { name: "x", ctrl: true });
  assert.equal(run(hidden, { name: "b" }, { sidebarShown: false }).calls.status, "Sidebar hidden.");
});

test("侧栏快捷键：M/A 切换对应区块并按展开状态给文案", () => {
  const files = createSidebarShortcutState();
  run(files, { name: "x", ctrl: true });
  assert.deepEqual(run(files, { name: "m" }), {
    result: true,
    calls: { consumed: true, section: "modifiedFiles", status: "Modified files expanded." },
  });

  const apis = createSidebarShortcutState();
  run(apis, { name: "x", ctrl: true });
  assert.deepEqual(run(apis, { name: "a" }), {
    result: true,
    calls: { consumed: true, section: "apis", status: "APIs expanded." },
  });

  const collapsed = createSidebarShortcutState();
  run(collapsed, { name: "x", ctrl: true });
  assert.equal(
    run(collapsed, { name: "m" }, { filesExpanded: false }).calls.status,
    "Modified files collapsed.",
  );
  const apisCollapsed = createSidebarShortcutState();
  run(apisCollapsed, { name: "x", ctrl: true });
  assert.equal(
    run(apisCollapsed, { name: "a" }, { apisExpanded: false }).calls.status,
    "APIs collapsed.",
  );
});

test("侧栏快捷键：普通按键不消费也不改状态", () => {
  assert.deepEqual(run(createSidebarShortcutState(), { name: "z" }), {
    result: false,
    calls: {},
  });
});
