/**
 * M4 adapter：Command palette 结构、输入挂点与视觉 token。
 * 运行：pnpm --filter @mode/ui test
 */
import assert from "node:assert/strict";
import test from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";

import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandShortcut,
} from "../src/ui-foundation/adapters/command.js";
import {
  commandGroupClassName,
  commandInputClassName,
  commandItemClassName,
  commandShortcutClassName,
} from "../src/ui-foundation/commandStyles.js";
import { findUiTypographyViolations } from "../src/lib/uiFoundation.js";

function renderCommand(): string {
  return renderToStaticMarkup(
    createElement(
      Command,
      { shouldFilter: false, label: "Commands" },
      createElement(CommandInput, { placeholder: "Search", "aria-label": "Search" }),
      createElement(
        CommandList,
        null,
        createElement(CommandEmpty, null, "No results"),
        createElement(
          CommandGroup,
          { heading: "Actions" },
          createElement(
            CommandItem,
            { value: "rename" },
            "Rename",
            createElement(CommandShortcut, null, "F2"),
          ),
        ),
      ),
    ),
  );
}

test("Command adapter：渲染搜索输入、分组、条目与快捷键", () => {
  const html = renderCommand();
  assert.match(html, /data-slot="command-input"/);
  assert.match(html, /placeholder="Search"/);
  assert.match(html, /Actions/);
  assert.match(html, /Rename/);
  assert.match(html, /F2/);
  assert.match(html, /data-slot="command-shortcut"/);
  assert.match(html, /cmdk-empty/);
});

test("Command adapter：类名使用排版 token 且选中态映射到 menu-hover", () => {
  const classes = [
    commandInputClassName(),
    commandGroupClassName(),
    commandItemClassName(),
    commandShortcutClassName(),
  ];
  assert.deepEqual(findUiTypographyViolations(classes), []);
  assert.match(commandItemClassName(), /data-\[selected=true\]:bg-menu-hover/);
  assert.match(commandShortcutClassName(), /text-ui-xs/);
});
