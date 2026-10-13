// 已保存工作流的协议 schema（docs/dynamic-workflow/launch.md「Data path」+
// docs/dynamic-workflow/launch.md「Data path」）。
import { describe, expect, it } from "vitest";
import {
  MODE_WORKFLOWS_RUNS_MAX_LIMIT,
  modeProtocolMethods,
  modeSavedWorkflowMetaSchema,
  modeSavedWorkflowScopeSchema,
  modeWorkflowsDeleteParamsSchema,
  modeWorkflowsDeleteResultSchema,
  modeWorkflowsGetParamsSchema,
  modeWorkflowsGetResultSchema,
  modeWorkflowsListParamsSchema,
  modeWorkflowsListResultSchema,
  modeWorkflowsMoveParamsSchema,
  modeWorkflowsMoveResultSchema,
  modeWorkflowsRunsParamsSchema,
  modeWorkflowsRunsResultSchema,
  modeWorkflowsUpdateMetaParamsSchema,
  modeWorkflowsUpdateMetaResultSchema,
  MODE_WORKFLOWS_FOR_RUN_MAX_CANDIDATES,
  modeWorkflowsForRunParamsSchema,
  modeWorkflowsForRunResultSchema,
  modeWorkflowsSaveParamsSchema,
  modeWorkflowsSaveResultSchema,
} from "../src/mode-protocol/index.js";

// workspace ref 要带 workspaceKey（services 层 resolveWorkspaceKey 补上；协议层不猜）。
const workspace = { workspacePath: "/repo", workspaceKey: "/repo" };

describe("mode-protocol saved workflows", () => {
  it("方法名固定，都是 workspace 级、无 session 的方法", () => {
    expect(modeProtocolMethods.workflowsList).toBe("workflows/list");
    expect(modeProtocolMethods.workflowsGet).toBe("workflows/get");
    expect(modeProtocolMethods.workflowsUpdateMeta).toBe("workflows/updateMeta");
    expect(modeProtocolMethods.workflowsDelete).toBe("workflows/delete");
    expect(modeProtocolMethods.workflowsRuns).toBe("workflows/runs");
    expect(modeProtocolMethods.workflowsMove).toBe("workflows/move");
  });

  it("scope 枚举两档：project / global，拒绝别的值", () => {
    expect(modeSavedWorkflowScopeSchema.parse("project")).toBe("project");
    expect(modeSavedWorkflowScopeSchema.parse("global")).toBe("global");
    // skills / subagents 那一档叫 user，本特性有意分道——不接受 user。
    expect(() => modeSavedWorkflowScopeSchema.parse("user")).toThrow();
  });

  it("list：结果里的条目不含脚本正文，坏文件单列 invalid，dir 必填", () => {
    expect(modeWorkflowsListParamsSchema.parse({ workspace })).toEqual({ workspace });
    // scope 可选，接受 global，拒绝未知值。
    expect(modeWorkflowsListParamsSchema.parse({ workspace, scope: "global" }).scope).toBe(
      "global",
    );
    expect(() => modeWorkflowsListParamsSchema.parse({ workspace, scope: "user" })).toThrow();
    const result = modeWorkflowsListResultSchema.parse({
      workflows: [
        {
          name: "x",
          description: "d",
          scope: "global",
          path: "/home/me/.mode/workflows/x.dwf.ts",
          args: { branch: { type: "string", required: true } },
        },
      ],
      invalid: [{ path: "/repo/.mode/workflows/bad.dwf.ts", reason: "invalid_yaml" }],
      dir: "/home/me/.mode/workflows",
    });
    expect(result.workflows[0]?.args?.branch?.required).toBe(true);
    expect(result.dir).toBe("/home/me/.mode/workflows");
    // dir 缺席即拒（GUI 的文件监听没有它就无处可 watch）。
    expect(() =>
      modeWorkflowsListResultSchema.parse({
        workflows: [],
        invalid: [],
      }),
    ).toThrow();
    // 条目里混进脚本正文仍拒。
    expect(() =>
      modeWorkflowsListResultSchema.parse({
        workflows: [{ name: "x", description: "d", scope: "project", path: "/p", script: "…" }],
        invalid: [],
        dir: "/repo/.mode/workflows",
      }),
    ).toThrow();
  });

  it("get：scope 可选；ok 分支带 meta + script；失败分支 reason 四态可分辨", () => {
    expect(modeWorkflowsGetParamsSchema.parse({ workspace, name: "x" })).toEqual({
      workspace,
      name: "x",
    });
    expect(
      modeWorkflowsGetParamsSchema.parse({ workspace, name: "x", scope: "global" }).scope,
    ).toBe("global");
    expect(() =>
      modeWorkflowsGetParamsSchema.parse({ workspace, name: "x", scope: "user" }),
    ).toThrow();
    const ok = modeWorkflowsGetResultSchema.parse({
      ok: true,
      name: "x",
      path: "/p",
      scope: "global",
      meta: { description: "d", whenToUse: "w" },
      script: "export default async () => {}",
    });
    expect(ok.ok).toBe(true);
    for (const reason of ["invalid_name", "not_found", "parse_error", "read_error"]) {
      expect(modeWorkflowsGetResultSchema.parse({ ok: false, reason })).toEqual({
        ok: false,
        reason,
      });
    }
    expect(() => modeWorkflowsGetResultSchema.parse({ ok: false, reason: "boom" })).toThrow();
  });

  it("updateMeta：scope 可选；meta 复用 contracts 的 meta schema，strict 拒未知键", () => {
    const meta = { description: "d", args: { n: { type: "number", default: 3 } } };
    expect(modeWorkflowsUpdateMetaParamsSchema.parse({ workspace, name: "x", meta })).toEqual({
      workspace,
      name: "x",
      meta,
    });
    expect(
      modeWorkflowsUpdateMetaParamsSchema.parse({ workspace, name: "x", meta, scope: "global" })
        .scope,
    ).toBe("global");
    expect(() => modeSavedWorkflowMetaSchema.parse({ description: "" })).toThrow();
    expect(() => modeSavedWorkflowMetaSchema.parse({ description: "d", script: "x" })).toThrow();
    expect(() =>
      modeWorkflowsUpdateMetaParamsSchema.parse({ workspace, name: "x", meta, script: "x" }),
    ).toThrow();
    expect(modeWorkflowsUpdateMetaResultSchema.parse({ ok: true, path: "/p" })).toEqual({
      ok: true,
      path: "/p",
    });
  });

  it("delete：scope 可选；params 只有 workspace + name (+ scope)；结果 ok/failure 二态", () => {
    expect(modeWorkflowsDeleteParamsSchema.parse({ workspace, name: "x" })).toEqual({
      workspace,
      name: "x",
    });
    expect(
      modeWorkflowsDeleteParamsSchema.parse({ workspace, name: "x", scope: "global" }).scope,
    ).toBe("global");
    expect(() => modeWorkflowsDeleteParamsSchema.parse({ workspace, name: "" })).toThrow();
    expect(modeWorkflowsDeleteResultSchema.parse({ ok: false, reason: "not_found" })).toEqual({
      ok: false,
      reason: "not_found",
    });
  });

  it("runs：scope 可选；limit 1..50、name 可选；行带归属字段 / cwd / args；truncated 只能是 true", () => {
    expect(modeWorkflowsRunsParamsSchema.parse({ workspace, limit: 1 })).toEqual({
      workspace,
      limit: 1,
    });
    expect(
      modeWorkflowsRunsParamsSchema.parse({ workspace, limit: 1, scope: "global" }).scope,
    ).toBe("global");
    expect(() =>
      modeWorkflowsRunsParamsSchema.parse({ workspace, limit: 1, scope: "user" }),
    ).toThrow();
    expect(
      modeWorkflowsRunsParamsSchema.parse({
        workspace,
        name: "x",
        limit: MODE_WORKFLOWS_RUNS_MAX_LIMIT,
      }).name,
    ).toBe("x");
    expect(() => modeWorkflowsRunsParamsSchema.parse({ workspace, limit: 0 })).toThrow();
    expect(() =>
      modeWorkflowsRunsParamsSchema.parse({
        workspace,
        limit: MODE_WORKFLOWS_RUNS_MAX_LIMIT + 1,
      }),
    ).toThrow();
    expect(() => modeWorkflowsRunsParamsSchema.parse({ workspace, limit: 1.5 })).toThrow();
    const result = modeWorkflowsRunsResultSchema.parse({
      runs: [
        {
          runId: "r1",
          name: "x",
          status: "running",
          createdAt: 1,
          updatedAt: 2,
          spentTokens: 0,
          parentSessionId: "s1",
          toolCallId: "t1",
          args: { branch: "main" },
          cwd: "/repo/one",
        },
        { runId: "r2", status: "errored", createdAt: 1, updatedAt: 2, spentTokens: 10 },
      ],
      truncated: true,
    });
    expect(result.runs).toHaveLength(2);
    // 全局变体的行带 cwd（GUI 用它标项目）；老行 / 项目变体可缺。
    expect(result.runs[0]?.cwd).toBe("/repo/one");
    expect(result.runs[1]).not.toHaveProperty("cwd");
    expect(result.truncated).toBe(true);
    expect(() => modeWorkflowsRunsResultSchema.parse({ runs: [], truncated: false })).toThrow();
    expect(modeWorkflowsRunsResultSchema.parse({ runs: [] })).toEqual({ runs: [] });
  });

  // 用户面产物的中枢摘要（docs/dynamic-workflow/authoring.md「How the user sees them」）。
  // optional，照 `cwd` 的先例：老 CLI 不发这个键，少一个键是退化不是错误。
  it("runs：行的 artifacts 可选、≤ 8 件、只认六个 kind，老行缺席仍可解析", () => {
    const withArtifacts = modeWorkflowsRunsResultSchema.parse({
      runs: [
        {
          runId: "r1",
          status: "completed",
          createdAt: 1,
          updatedAt: 2,
          spentTokens: 0,
          artifacts: [
            { id: "report", kind: "file", title: "审计报告", version: 2, contentType: "application/pdf" },
            // 预置看板没有 contentType；title 也可缺（facade 的默认标题就是 id）。
            { id: "perf", kind: "chart", version: 1 },
          ],
        },
        // 老 CLI / 无产物的 run：整字段缺席，不是空数组。
        { runId: "r2", status: "errored", createdAt: 1, updatedAt: 2, spentTokens: 10 },
      ],
    });
    expect(withArtifacts.runs[0]?.artifacts).toHaveLength(2);
    expect(withArtifacts.runs[0]?.artifacts?.[0]?.kind).toBe("file");
    expect(withArtifacts.runs[0]?.artifacts?.[1]).not.toHaveProperty("contentType");
    expect(withArtifacts.runs[1]).not.toHaveProperty("artifacts");

    const row = (artifacts: unknown) => ({
      runs: [{ runId: "r1", status: "completed", createdAt: 1, updatedAt: 2, spentTokens: 0, artifacts }],
    });
    // 上界 8：chips 一行画得下的数量，与通知载荷同值。
    const nine = Array.from({ length: 9 }, (_, index) => ({
      id: `a${index}`,
      kind: "markdown" as const,
      version: 1,
    }));
    expect(() => modeWorkflowsRunsResultSchema.parse(row(nine))).toThrow();
    expect(modeWorkflowsRunsResultSchema.parse(row(nine.slice(0, 8))).runs[0]?.artifacts).toHaveLength(8);
    // 未知 kind 与未知键都拒（元素 strict）：GUI 按 kind 画图标，没有兜底图标可用。
    expect(() => modeWorkflowsRunsResultSchema.parse(row([{ id: "a", kind: "video", version: 1 }]))).toThrow();
    expect(() =>
      modeWorkflowsRunsResultSchema.parse(row([{ id: "a", kind: "file", version: 1, bytes: 12 }])),
    ).toThrow();
    // id 非空、version 必填。
    expect(() => modeWorkflowsRunsResultSchema.parse(row([{ id: "", kind: "file", version: 1 }]))).toThrow();
    expect(() => modeWorkflowsRunsResultSchema.parse(row([{ id: "a", kind: "file" }]))).toThrow();
  });

  it("move：params 只有 workspace/name（无 to，全局→项目单向）；结果 ok 分支带 from/to，失败分支 reason 五态", () => {
    expect(modeWorkflowsMoveParamsSchema.parse({ workspace, name: "x" })).toEqual({
      workspace,
      name: "x",
    });
    // 2026-09-04 追记：项目→全局改为模型概括，`to` 参数删除；带上即 strict 拒。
    expect(() =>
      modeWorkflowsMoveParamsSchema.parse({ workspace, name: "x", to: "global" }),
    ).toThrow();
    expect(() =>
      modeWorkflowsMoveParamsSchema.parse({ workspace, name: "x", to: "project" }),
    ).toThrow();

    const ok = modeWorkflowsMoveResultSchema.parse({
      ok: true,
      from: "/home/me/.mode/workflows/x.dwf.ts",
      to: "/repo/.mode/workflows/x.dwf.ts",
    });
    expect(ok).toEqual({
      ok: true,
      from: "/home/me/.mode/workflows/x.dwf.ts",
      to: "/repo/.mode/workflows/x.dwf.ts",
    });
    for (const reason of [
      "invalid_name",
      "not_found",
      "target_exists",
      "read_error",
      "write_error",
    ]) {
      expect(
        modeWorkflowsMoveResultSchema.parse({ ok: false, reason, path: "/p", detail: "why" }),
      ).toEqual({ ok: false, reason, path: "/p", detail: "why" });
    }
    // path / detail 可缺；未知 reason 拒。
    expect(modeWorkflowsMoveResultSchema.parse({ ok: false, reason: "target_exists" })).toEqual({
      ok: false,
      reason: "target_exists",
    });
    expect(() => modeWorkflowsMoveResultSchema.parse({ ok: false, reason: "boom" })).toThrow();
    // ok 分支缺 from/to 即拒；多出旧的 scope 也拒（strict）。
    expect(() => modeWorkflowsMoveResultSchema.parse({ ok: true, from: "/a" })).toThrow();
    expect(() =>
      modeWorkflowsMoveResultSchema.parse({ ok: true, from: "/a", to: "/b", scope: "project" }),
    ).toThrow();
  });
});

// 完成卡的两个方法（docs/dynamic-workflow/transcript-and-notifications.md「Saving the run, and
// running it again」）。
describe("mode-protocol saving a run", () => {
  it("方法名固定", () => {
    expect(modeProtocolMethods.workflowsSave).toBe("workflows/save");
    expect(modeProtocolMethods.workflowsForRun).toBe("workflows/forRun");
  });

  it("save 入参：runId 取代脚本（不收 script），workspace 必带，scope / overwrite 可缺；strict", () => {
    const params = { workspace, runId: "run-1", name: "pr-review", meta: { description: "d" } };
    expect(modeWorkflowsSaveParamsSchema.parse(params)).toEqual(params);
    expect(
      modeWorkflowsSaveParamsSchema.parse({ ...params, scope: "global", overwrite: true }),
    ).toMatchObject({ scope: "global", overwrite: true });
    expect(() => modeWorkflowsSaveParamsSchema.parse({ ...params, script: "x" })).toThrow();
    const { workspace: _dropped, ...withoutWorkspace } = params;
    expect(() => modeWorkflowsSaveParamsSchema.parse(withoutWorkspace)).toThrow();
    expect(() => modeWorkflowsSaveParamsSchema.parse({ ...params, runId: "" })).toThrow();
  });

  it("save 结果：ok 分支带 overwritten 与可选的遮蔽方向；拒绝词表封闭", () => {
    expect(
      modeWorkflowsSaveResultSchema.parse({
        ok: true,
        name: "x",
        scope: "project",
        path: "/p",
        overwritten: false,
        shadowing: "hides_global",
      }),
    ).toMatchObject({ shadowing: "hides_global" });
    for (const reason of [
      "invalid_name",
      "run_not_found",
      "script_missing",
      "target_exists",
      "compile_failed",
      "write_error",
    ]) {
      expect(modeWorkflowsSaveResultSchema.parse({ ok: false, reason })).toEqual({
        ok: false,
        reason,
      });
    }
    expect(() => modeWorkflowsSaveResultSchema.parse({ ok: false, reason: "boom" })).toThrow();
  });

  it("forRun：候选有上界；结果三个字段全可缺（全缺即「没保存、也不知道实参」）", () => {
    const candidates = Array.from({ length: MODE_WORKFLOWS_FOR_RUN_MAX_CANDIDATES }, (_, i) => ({
      name: `w${i}`,
    }));
    expect(
      modeWorkflowsForRunParamsSchema.parse({ workspace, runId: "r", candidates }).candidates,
    ).toHaveLength(MODE_WORKFLOWS_FOR_RUN_MAX_CANDIDATES);
    expect(() =>
      modeWorkflowsForRunParamsSchema.parse({
        workspace,
        runId: "r",
        candidates: [...candidates, { name: "one-too-many" }],
      }),
    ).toThrow();
    expect(modeWorkflowsForRunResultSchema.parse({})).toEqual({});
    expect(
      modeWorkflowsForRunResultSchema.parse({
        entry: { name: "x", description: "d", scope: "project", path: "/p" },
        match: "script",
        runArgs: { base: "main" },
      }),
    ).toMatchObject({ match: "script", runArgs: { base: "main" } });
    expect(() => modeWorkflowsForRunResultSchema.parse({ match: "guess" })).toThrow();
  });
});
