import { resolveExecutionState } from "@mode/shared";
import { submissionModeSchema, type SessionConfigState } from "@mode/shared/mode-protocol-v4";
import type { ModelSelectionView } from "@mode/services";
import { V4_DRAFT_SCOPE_ROOT, type V4ComposerDraft } from "@/v4/composer/composerDraftStore.js";
import { initializeNewTaskDraft } from "@/v4/composer/newTaskDraft.js";

/** 草稿 scope：真实会话优先，其次面板显式 scope，最后才是主新任务。 */
export function resolveComposerDraftScopeId(
  sessionId: string | null,
  draftScopeIdOverride?: string,
): string {
  return sessionId ?? (draftScopeIdOverride?.trim() || V4_DRAFT_SCOPE_ROOT);
}

/**
 * 用 Seed 初始化尚未有 mode 的草稿。
 *
 * 原因：会话 Snapshot 可能晚于用户第一次选模到达。此时草稿里已有 modelSelection
 * 但没有 mode；初始化只能补缺失字段，不能把用户刚选的模型换回 Snapshot 的旧值。
 * 返回 null 表示当前不需要初始化。
 */
export function initializeComposerDraftFromSeed(params: {
  draft: V4ComposerDraft;
  initializeAsNewTask: boolean;
  modelSelectionView: ModelSelectionView | null;
  sessionConfig: Partial<SessionConfigState> | null;
  workspacePath: string;
  workspaceIdentity?: string;
}): V4ComposerDraft | null {
  const { draft, initializeAsNewTask, modelSelectionView, sessionConfig } = params;
  if (draft.mode) return null;
  if (initializeAsNewTask ? modelSelectionView === null : sessionConfig == null) return null;

  const seeded =
    initializeAsNewTask && modelSelectionView
      ? initializeNewTaskDraft(
          draft,
          params.workspacePath,
          params.workspaceIdentity,
          modelSelectionView,
        )
      : (() => {
          const mode = submissionModeSchema.safeParse(sessionConfig?.mode);
          return {
            ...draft,
            mode: mode.success && mode.data !== "plan" ? mode.data : "build",
            planEnabled: resolveExecutionState(sessionConfig ?? {}).planEnabled,
            modelSelection: sessionConfig?.modelSelection,
          } satisfies V4ComposerDraft;
        })();

  return draft.modelSelection ? { ...seeded, modelSelection: draft.modelSelection } : seeded;
}

/**
 * 会话真值回填（spec 规则 8，2026-10-25 重启丢模型修复）：已有会话的草稿
 * modelSelection 为空而 Snapshot config 带选择时，把会话真值填进草稿。
 * 草稿持久化是 best-effort localStorage（配额/键丢失/历史版本都可能丢选择），
 * agent 会话配置才是既有会话的权威事实；不回填则重启后选择器永远回落默认模型。
 * 只填空草稿——已有任何显式选择（含与快照不同）一律不动（规则 2）。
 */
export function reconcileMissingDraftModelSelection(
  draft: V4ComposerDraft,
  sessionConfig: { modelSelection?: V4ComposerDraft["modelSelection"] } | null,
): V4ComposerDraft | null {
  if (draft.modelSelection !== undefined) {
    return null;
  }
  const selection = sessionConfig?.modelSelection;
  if (!selection) {
    return null;
  }
  return { ...draft, modelSelection: selection };
}

/** 带初始 Prompt 新建任务时只替换正文，保留已有配置意图。 */
export function buildInitialPromptComposerDraft(
  existing: V4ComposerDraft | null,
  text: string,
  mention?: V4ComposerDraft["mention"],
): Omit<V4ComposerDraft, "updatedAt"> {
  return {
    ...(existing?.mode ? { mode: existing.mode } : {}),
    ...(existing?.planEnabled !== undefined ? { planEnabled: existing.planEnabled } : {}),
    ...(existing?.modelSelection ? { modelSelection: existing.modelSelection } : {}),
    ...(existing?.lastPlanTransitionId
      ? { lastPlanTransitionId: existing.lastPlanTransitionId }
      : {}),
    ...(existing?.lastPermissionGrantId
      ? { lastPermissionGrantId: existing.lastPermissionGrantId }
      : {}),
    ...(existing?.initializeFromNewTask ? { initializeFromNewTask: true as const } : {}),
    text,
    ...(mention ? { mention } : {}),
  };
}
