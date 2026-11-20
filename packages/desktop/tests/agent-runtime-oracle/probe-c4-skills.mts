/* oxlint-disable eslint(max-lines) -- 目录/注入/触发三组观察共享同一套投影，拆散会复制规则。 */
// C4 Task 0 观察项 2：Skills 的黑盒观察。
//
// 用法：pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-c4-skills.mts
//
// 观察项：
//   1. workspace 级 skill 的发现位置（`.mode/skills/` vs `.claude/skills/`，黑盒对照）；
//   2. `skills/referenceCatalog`（workspace 与 session 两种 authority）的条目形状；
//   3. skill 注入形状：模型请求 system 段与合成 user 提醒消息的原文形态；
//   4. skill 触发：`Skill` 工具调用的事件与工具结果内容；
//   5. `plugins/referenceCatalog` 与 `plugins/resolveSuggestedReference`（引用解析）。
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { JsonObject } from "./session-lifecycle-fixtures.mjs";
import {
  isolationRecord,
  startC4Harness,
  writeC4Fixture,
  writeStdioMcpServer,
} from "./probe-c4-support.mjs";

const SEED = { "hello.txt": "hello\n" };

const WORKSPACE_SKILL_BODY = [
  "---",
  "name: probe-workspace-skill",
  "description: Use when the C4 probe asks for the workspace skill path discovery.",
  "---",
  "",
  "Workspace skill body for the C4 probe.",
  "",
].join("\n");

const CLAUDE_SKILL_BODY = [
  "---",
  "name: probe-claude-skill",
  "description: Use when the C4 probe checks legacy .claude skill discovery.",
  "---",
  "",
  "Claude directory skill body for the C4 probe.",
  "",
].join("\n");

/** 候选位置一次种两个，由 catalog 结果裁决哪个位置被发现（黑盒对照）。 */
async function seedSkillCandidates(cwd: string): Promise<void> {
  const { mkdir, writeFile } = await import("node:fs/promises");
  const modeSkill = join(cwd, ".mode", "skills", "probe-workspace-skill", "SKILL.md");
  await mkdir(join(modeSkill, ".."), { recursive: true });
  await writeFile(modeSkill, WORKSPACE_SKILL_BODY, "utf8");
  const claudeSkill = join(cwd, ".claude", "skills", "probe-claude-skill", "SKILL.md");
  await mkdir(join(claudeSkill, ".."), { recursive: true });
  await writeFile(claudeSkill, CLAUDE_SKILL_BODY, "utf8");
}

/** 从 mock 请求体提取注入观察：system 段里的 skill 提及 + 合成 user 提醒消息原文。 */
/**
 * 从 mock 请求体提取注入观察：system 段里的 skill 提及 + 合成 user 提醒消息原文。
 * 实测（见契约 §2）：无 MCP 时首个请求没有 skills 提醒（末条 user 是日期 system-reminder）；
 * 带 MCP server 时（工具面在物化期晚到）首个请求末条 user 是 skills 提醒——两种形状都要录。
 */
function projectInjection(
  bodies: readonly JsonObject[],
  normalize: (value: string) => string,
): JsonObject {
  const first = bodies[0] ?? {};
  const system = Array.isArray(first.system) ? (first.system as JsonObject[]) : [];
  const systemTexts = system
    .map((block) => (typeof block.text === "string" ? block.text : ""))
    .filter((text) => text.length > 0);
  const reminders: JsonObject[] = [];
  for (const [index, body] of bodies.entries()) {
    const messages = Array.isArray(body.messages) ? (body.messages as JsonObject[]) : [];
    for (const message of messages) {
      if (message.role !== "user") continue;
      const content = message.content;
      const text =
        typeof content === "string"
          ? content
          : Array.isArray(content)
            ? (content as JsonObject[])
                .map((part) => (typeof part.text === "string" ? part.text : ""))
                .join("\n")
            : "";
      if (text.includes("skills are available for use with the Skill tool")) {
        reminders.push({ requestIndex: index, text: normalize(text).slice(0, 1600) });
      }
    }
  }
  const messages0 = Array.isArray(first.messages) ? (first.messages as JsonObject[]) : [];
  const lastUser0 = (() => {
    for (let index = messages0.length - 1; index >= 0; index -= 1) {
      const message = messages0[index];
      if (message.role !== "user") continue;
      const content = message.content;
      return typeof content === "string"
        ? content
        : Array.isArray(content)
          ? (content as JsonObject[])
              .map((part) => (typeof part.text === "string" ? part.text : ""))
              .join("\n")
          : "";
    }
    return "";
  })();
  return {
    systemBlocks: systemTexts.map((text) => ({
      chars: text.length,
      mentionsProbeSkill:
        text.includes("probe-workspace-skill") || text.includes("probe-claude-skill"),
    })),
    reminders,
    firstRequestLastUserHead: normalize(lastUser0).slice(0, 240),
  };
}

/** Case 1：目录 + 注入形状（发现位置裁决、两种 authority、注入原文）。 */
async function caseCatalogAndInjection(): Promise<JsonObject> {
  const dataRoot = await mkdtemp(join(tmpdir(), "mode-c4-skill-catalog-"));
  const cwd = await mkdtemp(join(tmpdir(), "mode-c4-skill-catalog-ws-"));
  await seedSkillCandidates(cwd);
  const mcpServerPath = await writeStdioMcpServer(dataRoot, "skill-cat", {
    serverName: "probe-skill-cat",
  });
  const harness = await startC4Harness({
    prefix: "c4-skill-catalog",
    seed: SEED,
    dataRoot,
    cwd,
    script: [{ text: "skills catalog done" }],
    // 带 MCP server 复现「skills 提醒注入」的触发条件（无 MCP 的对照见 firstRequestLastUserHead）。
    sessionCreateParams: {
      mcpServers: [
        { name: "probe-skill-cat", command: process.execPath, args: [mcpServerPath], env: [] },
      ],
    },
  });
  try {
    const workspaceCatalog = await harness.tryRequest("skills/referenceCatalog", {
      workspace: harness.workspaceRef(),
    });
    const beforeTurnSessionCatalog = await harness.tryRequest("skills/referenceCatalog", {
      workspace: harness.workspaceRef(),
      sessionId: harness.session.sessionId,
    });
    const pluginCatalog = await harness.tryRequest("plugins/referenceCatalog", {
      workspace: harness.workspaceRef(),
    });
    await harness.sendAndWait("plain prompt");
    const sessionCatalog = await harness.tryRequest("skills/referenceCatalog", {
      workspace: harness.workspaceRef(),
      sessionId: harness.session.sessionId,
    });
    const missingSessionError = await harness.tryRequest("skills/referenceCatalog", {
      workspace: harness.workspaceRef(),
      sessionId: "sess_missing",
    });
    const injection = projectInjection(harness.rawBodies, (value) =>
      String(harness.normalizeForFixture(value)),
    );
    return {
      isolation: isolationRecord(harness),
      workspaceCatalog,
      beforeTurnSessionCatalog,
      sessionCatalog,
      missingSessionError,
      pluginCatalogEntryCount: Array.isArray(
        (pluginCatalog.result as JsonObject | undefined)?.plugins,
      )
        ? ((pluginCatalog.result as JsonObject).plugins as unknown[]).length
        : null,
      pluginCatalogPluginNames: Array.isArray(
        (pluginCatalog.result as JsonObject | undefined)?.plugins,
      )
        ? ((pluginCatalog.result as JsonObject).plugins as JsonObject[]).map(
            (entry) => entry.pluginId,
          )
        : null,
      injection,
      // 探针自己算出的路径也走同一套规范化（cwd 折叠成 <cwd>），避免夹具写死临时目录。
      workspaceModeSkillFile: harness.normalizeForFixture(
        join(cwd, ".mode", "skills", "probe-workspace-skill", "SKILL.md"),
      ),
      claudeSkillFile: harness.normalizeForFixture(
        join(cwd, ".claude", "skills", "probe-claude-skill", "SKILL.md"),
      ),
    };
  } finally {
    await harness.stop();
  }
}

/** Case 2：skill 触发（Skill 工具调用 → 事件 → 工具结果注入 SKILL.md 正文）。 */
async function caseSkillTrigger(): Promise<JsonObject> {
  const dataRoot = await mkdtemp(join(tmpdir(), "mode-c4-skill-trigger-"));
  const cwd = await mkdtemp(join(tmpdir(), "mode-c4-skill-trigger-ws-"));
  await seedSkillCandidates(cwd);
  const consumed = new Set<string>();
  const harness = await startC4Harness({
    prefix: "c4-skill-trigger",
    seed: SEED,
    dataRoot,
    cwd,
    script: [
      (body) => {
        const messages = Array.isArray(body.messages) ? (body.messages as JsonObject[]) : [];
        const texts: string[] = [];
        for (const message of messages) {
          if (message.role !== "user") continue;
          if (typeof message.content === "string") texts.push(message.content);
          else if (Array.isArray(message.content)) {
            texts.push(
              (message.content as JsonObject[])
                .map((part) => (typeof part.text === "string" ? part.text : ""))
                .join("\n"),
            );
          }
        }
        const has = (needle: string): boolean => texts.some((text) => text.includes(needle));
        if (has("trigger the skill") && !consumed.has("skill")) {
          consumed.add("skill");
          return { toolCalls: [{ name: "Skill", input: { skill: "probe-workspace-skill" } }] };
        }
        if (consumed.has("skill")) return { text: "skill trigger done" };
        return { text: "default answer" };
      },
    ],
  });
  try {
    const start = harness.eventCount();
    await harness.session.send("trigger the skill please");
    await harness.waitForEvent(
      start,
      (event) => {
        const payload = (event.payload ?? {}) as JsonObject;
        return event.type === "tool.updated" && payload.kind === "result";
      },
      "Skill tool result",
      60_000,
    );
    await harness.waitForEvent(
      start,
      (event) => event.type === "turn.completed",
      "skill trigger turn.completed",
      60_000,
    );
    const events = harness.eventsSince(start);
    // Skill 工具结果之后的模型请求里应包含 SKILL.md 正文（观察 skill 展开机制）。
    const followUpRequests = harness.rawBodies.length;
    const lastBody = harness.rawBodies.at(-1) ?? {};
    const messages = Array.isArray(lastBody.messages) ? (lastBody.messages as JsonObject[]) : [];
    const toolResultTexts = messages
      .filter((message) => message.role === "user" && Array.isArray(message.content))
      .flatMap((message) => message.content as JsonObject[])
      .filter((part) => part.type === "tool_result")
      .map((part) => {
        const content = part.content;
        if (typeof content === "string") return content;
        if (Array.isArray(content)) {
          return (content as JsonObject[])
            .map((inner) => (typeof inner.text === "string" ? inner.text : ""))
            .join("\n");
        }
        return "";
      });
    return {
      isolation: isolationRecord(harness),
      events,
      modelRequestCount: followUpRequests,
      skillToolResultTexts: toolResultTexts.map((text) =>
        String(harness.normalizeForFixture(text)).slice(0, 400),
      ),
      toolResultContainsSkillBody: toolResultTexts.some((text) =>
        text.includes("Workspace skill body"),
      ),
    };
  } finally {
    await harness.stop();
  }
}

/** Case 3：引用解析——resolveSuggestedReference 对官方目录与未知 stableId 的形状。 */
async function caseReferenceResolution(): Promise<JsonObject> {
  const harness = await startC4Harness({
    prefix: "c4-skill-resolve",
    seed: SEED,
    script: [{ text: "reference resolve done" }],
  });
  try {
    const bareName = await harness.tryRequest("plugins/resolveSuggestedReference", {
      workspace: harness.workspaceRef(),
      stableId: "browser-use",
      operationId: "op-c4-resolve-bare",
      clientMode: "desktop-continuous",
      deliveryKind: "desktop-continuous",
    });
    const qualified = await harness.tryRequest("plugins/resolveSuggestedReference", {
      workspace: harness.workspaceRef(),
      stableId: "browser-use@mode-plugins-official",
      operationId: "op-c4-resolve-qualified",
      clientMode: "desktop-continuous",
      deliveryKind: "desktop-continuous",
    });
    const available = await harness.tryRequest("plugins/resolveSuggestedReference", {
      workspace: harness.workspaceRef(),
      stableId: "mimosa@mode-plugins-official",
      operationId: "op-c4-resolve-available",
      clientMode: "desktop-continuous",
      deliveryKind: "desktop-continuous",
    });
    const unknown = await harness.tryRequest("plugins/resolveSuggestedReference", {
      workspace: harness.workspaceRef(),
      stableId: "nonexistent",
      operationId: "op-c4-resolve-unknown",
      clientMode: "desktop-continuous",
      deliveryKind: "desktop-continuous",
    });
    const missingParams = await harness.tryRequest("plugins/resolveSuggestedReference", {
      workspace: harness.workspaceRef(),
    });
    return {
      isolation: isolationRecord(harness),
      bareName,
      qualified,
      available,
      unknown,
      missingParams,
    };
  } finally {
    await harness.stop();
  }
}

const fixtures: Record<string, JsonObject> = {
  "skills-catalog-injection": {
    case: "skills-catalog-injection",
    note:
      "C4 Task 0 oracle 捕获：workspace skill 的发现位置黑盒对照（.mode/skills vs .claude/skills）、" +
      "skills/referenceCatalog 的 workspace/session authority 条目形状、未知 sessionId 的 fail-closed、" +
      "plugins/referenceCatalog 的插件条目，以及首个模型请求里 skill 注入（system 段与合成 user 提醒）。",
    input: {
      seed: SEED,
      seededSkills: [
        ".mode/skills/probe-workspace-skill/SKILL.md",
        ".claude/skills/probe-claude-skill/SKILL.md",
      ],
      prompt: "plain prompt",
    },
    expect: await caseCatalogAndInjection(),
  },
  "skills-trigger": {
    case: "skills-trigger",
    note:
      "C4 Task 0 oracle 捕获：Skill 工具调用（script 驱动）的事件序列与工具结果内容——" +
      "SKILL.md 正文经工具结果展开进下一轮模型请求。",
    input: {
      seed: SEED,
      prompt: "trigger the skill please",
      skillCall: { name: "Skill", input: { skill: "probe-workspace-skill" } },
    },
    expect: await caseSkillTrigger(),
  },
  "skills-reference-resolution": {
    case: "skills-reference-resolution",
    note:
      "C4 Task 0 oracle 捕获：plugins/resolveSuggestedReference 的 ready/unavailable 形状——" +
      "官方 bundled 插件名与未知 stableId 的对照、缺参数的 -32602。",
    input: {
      stableIds: ["browser-use", "nonexistent"],
    },
    expect: await caseReferenceResolution(),
  },
};

const targets: string[] = [];
for (const [name, fixture] of Object.entries(fixtures)) {
  targets.push(await writeC4Fixture(name, fixture));
}
process.stdout.write(
  `${JSON.stringify(
    {
      fixtures: targets,
      summaries: Object.fromEntries(
        Object.entries(fixtures).map(([name, fixture]) => {
          const expect = fixture.expect as JsonObject;
          const catalog = (expect.workspaceCatalog as JsonObject | undefined)?.result as
            | JsonObject
            | undefined;
          return [
            name,
            {
              workspaceSkillCount: Array.isArray(catalog?.skills) ? catalog.skills.length : null,
              skillNames: Array.isArray(catalog?.skills)
                ? (catalog.skills as JsonObject[]).map((entry) => entry.name)
                : null,
            },
          ];
        }),
      ),
    },
    null,
    2,
  )}\n`,
);
