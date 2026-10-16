import assert from "node:assert/strict";
import test from "node:test";

// 桌面→IM 镜像的附件口径（docs/specs/bot-outbound-target.md「桌面输入镜像的附件缺口」）：
// 镜像只同步文本；附件数量必须一路传到服务层，由服务层在 IM 侧显式告知"未同步"。
// 静默省略会让 IM 侧的读者以为这次提问是完整的——本项目把它当可见的错，不接受这种降级。
// 服务层闭包内无法直接单测，沿用 bot-outbound-target / bot-group-chat 的源码契约模式。
const sessionPaneSource = await import("node:fs/promises").then((fs) =>
  fs.readFile(new URL("../src/v4/SessionPane.tsx", import.meta.url), "utf8"),
);

test("镜像调用点必须把附件数量传下去", () => {
  assert.ok(sessionPaneSource, "无法读取 SessionPane 源码");
  const calls = sessionPaneSource!.match(/mirrorDesktopPromptToIm\([^)]*\)/gu) ?? [];
  assert.ok(calls.length >= 2, "至少要有两条镜像调用路径（新建会话 + 既有会话）");
  for (const call of calls) {
    assert.ok(
      call.includes("readyAttachments.length"),
      `镜像调用点必须传附件数量，否则服务层无从告知未同步：${call}`,
    );
  }
});
