import type { ModePersistedFileChange } from "@mode/shared";

/**
 * 从原始文件快照推导变更类型，供 bot 回复的变更摘要展示"新增 / 修改 / 删除"
 * （原先只报文件数 + 行数增删，新建文件和改过的文件在回复里长得一样）。
 *
 * 放在 bot 层而不是给共享摘要类型加字段：`ModeTaskChangedFileSummary` 是 v4 传输面
 * 的类型（`mode-protocol-v4` + `validation` + UI 侧另有一份同名推导），加字段要同步
 * 四处 schema 与两份生产者；而 bot 已经拿着 `ModeSessionFile` 的原始快照，本地推导即可。
 *
 * 聚合口径与 `buildPerTurnChangeSummaries` 对齐：同一文件同轮多次写入时，
 * 首个快照的 `beforeContent` 是原始内容、最后一个快照的 `afterContent` 是最终内容。
 */

export type BotFileChangeKind = "added" | "modified" | "deleted";

export function deriveBotFileChangeKinds(
  fileChanges: readonly ModePersistedFileChange[] | undefined,
  turnIndex: number,
): Map<string, BotFileChangeKind> {
  const kinds = new Map<string, BotFileChangeKind>();
  const turn = fileChanges?.find((item) => item.turnIndex === turnIndex);
  if (!turn) {
    return kinds;
  }
  const originals = new Map<string, string | null>();
  const finals = new Map<string, string>();
  for (const snapshot of turn.snapshots) {
    // 只认首个快照的 beforeContent（后续写入的 before 是上一次写入后的内容）。
    if (!originals.has(snapshot.path)) {
      originals.set(snapshot.path, snapshot.beforeContent);
    }
    finals.set(snapshot.path, snapshot.afterContent);
  }
  for (const [path, original] of originals) {
    const final = finals.get(path) ?? "";
    kinds.set(
      path,
      // 原本不存在 → 新增；最终内容为空（原本有内容）→ 删除；其余是修改。
      original === null ? "added" : final === "" ? "deleted" : "modified",
    );
  }
  return kinds;
}
