import { describe, expect, it } from "vitest";
import {
  appendModeStreamingToolInputDelta,
  buildModeStreamingToolInputPreview,
  markModeStreamingToolInputPreviewMaterialized,
  shouldMaterializeModeStreamingToolInputPreview,
} from "../src/streaming-tool-input-preview.js";

describe("streaming tool input preview", () => {
  it("extracts file path and growing content from partial JSON", () => {
    const first = appendModeStreamingToolInputDelta(
      undefined,
      '{"file_path":"src/app.ts","content":"line 1\\n',
    );
    const second = appendModeStreamingToolInputDelta(first, "line 2");
    const preview = buildModeStreamingToolInputPreview(second.rawInput);

    expect(first.deltaCount).toBe(1);
    expect(second.deltaCount).toBe(2);
    expect(preview.complete).toBe(false);
    expect(preview.input).toMatchObject({
      file_path: "src/app.ts",
      content: "line 1\nline 2",
    });
  });

  it("extracts a growing ExitPlanMode plan from partial JSON", () => {
    const preview = buildModeStreamingToolInputPreview(
      '{"plan":"# 实施计划\\n\\n- 第一步\\n- 第二',
    );

    expect(preview.complete).toBe(false);
    expect(preview.input).toEqual({
      plan: "# 实施计划\n\n- 第一步\n- 第二",
    });
  });

  it("prefers complete tool call input when available", () => {
    const preview = buildModeStreamingToolInputPreview('{"file_path":"old.ts"', {
      file_path: "src/final.ts",
      content: "done",
    });

    expect(preview.complete).toBe(true);
    expect(preview.input).toEqual({
      file_path: "src/final.ts",
      content: "done",
    });
  });

  it("extracts a user-facing title from partial tool input", () => {
    const preview = buildModeStreamingToolInputPreview(
      '{"code":"await openPage();","title":"打开目标页面',
    );

    expect(preview.complete).toBe(false);
    expect(preview.input).toMatchObject({ title: "打开目标页面" });
  });

  it("extracts a CreateWorkflow name and growing script from partial JSON", () => {
    // 工作流卡的流式草稿靠半截 script 扫站；name 先到就先显示名字而不是兜底词。
    const preview = buildModeStreamingToolInputPreview(
      '{"name":"Research pipeline","script":"phase(\\"plan\\");\\nconst p = agent(\\"pl',
    );

    expect(preview.complete).toBe(false);
    expect(preview.input).toMatchObject({
      name: "Research pipeline",
      script: 'phase("plan");\nconst p = agent("pl',
    });
  });

  it("budgets active preview materialization and keeps background as tombstone", () => {
    const first = appendModeStreamingToolInputDelta(undefined, '{"content":"a');
    expect(shouldMaterializeModeStreamingToolInputPreview(first)).toBe(true);
    markModeStreamingToolInputPreviewMaterialized(first, 1_000);

    const second = appendModeStreamingToolInputDelta(first, "b");
    expect(
      shouldMaterializeModeStreamingToolInputPreview(second, {
        now: 1_100,
      }),
    ).toBe(false);
    expect(
      shouldMaterializeModeStreamingToolInputPreview(second, {
        now: 2_000,
      }),
    ).toBe(true);
    expect(
      shouldMaterializeModeStreamingToolInputPreview(second, {
        mode: "background-summary",
        now: 2_000,
      }),
    ).toBe(false);
  });

  it.each(["Write", "Edit"])(
    "%s previews the first delta eagerly and subsequent deltas once per second",
    (toolName) => {
      const first = appendModeStreamingToolInputDelta(undefined, '{"content":"a');
      expect(shouldMaterializeModeStreamingToolInputPreview(first, { now: 1_000, toolName })).toBe(
        true,
      );
      markModeStreamingToolInputPreviewMaterialized(first, 1_000);

      const second = appendModeStreamingToolInputDelta(first, "b".repeat(9 * 1024));
      expect(
        shouldMaterializeModeStreamingToolInputPreview(second, { now: 1_999, toolName }),
      ).toBe(false);
      expect(
        shouldMaterializeModeStreamingToolInputPreview(second, { now: 2_000, toolName }),
      ).toBe(true);
    },
  );

  it("keeps the raw-growth shortcut for non-file tools", () => {
    const first = appendModeStreamingToolInputDelta(undefined, '{"command":"p');
    markModeStreamingToolInputPreviewMaterialized(first, 1_000);
    const second = appendModeStreamingToolInputDelta(first, "x".repeat(8 * 1024));

    expect(
      shouldMaterializeModeStreamingToolInputPreview(second, {
        now: 1_001,
        toolName: "Bash",
      }),
    ).toBe(true);
  });

  it("does not time-flush large active inputs before raw growth budget", () => {
    const first = appendModeStreamingToolInputDelta(undefined, "x".repeat(9 * 1024));
    expect(shouldMaterializeModeStreamingToolInputPreview(first)).toBe(true);
    markModeStreamingToolInputPreviewMaterialized(first, 1_000);

    const slowChunk = appendModeStreamingToolInputDelta(first, "y");
    expect(
      shouldMaterializeModeStreamingToolInputPreview(slowChunk, {
        now: 10_000,
      }),
    ).toBe(false);

    const grownChunk = appendModeStreamingToolInputDelta(first, "y".repeat(8 * 1024));
    expect(
      shouldMaterializeModeStreamingToolInputPreview(grownChunk, {
        now: 10_000,
      }),
    ).toBe(true);
  });
});
