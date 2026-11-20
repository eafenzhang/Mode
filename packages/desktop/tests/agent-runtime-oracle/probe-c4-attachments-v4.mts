/* oxlint-disable eslint(max-lines) -- 附件事务/v4 帧/只读查询三组观察共享同一会话生命周期，拆散会破坏时序。 */
// C4 Task 0 观察项 5+6：附件事务与 v4 会话帧的黑盒观察。
//
// 用法：pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-c4-attachments-v4.mts
//
// 时序：先 v4/conversation/subscribe（帧覆盖后续全部动作），再附件 begin/chunk/commit 全事务；
// 附件入会话的两条通路都观察——v4/command sendText（实测 FK 失败，oracle 缺陷登记）与
// 旧 session/send（accepted，但 ref 不进 userInput 行）；读面（attachmentRead /
// conversationAttachmentStat/Read / previewSource）对 committed ref 的授权错误形状；
// abort 事务；rowsRange/plans/usage/fileChanges/backgroundBashOutput 只读查询；
// resync/unsubscribe/sessions-index 尝试；session_input DB 行。
import { createHash } from "node:crypto";
import { readdir } from "node:fs/promises";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import type { JsonObject } from "./session-lifecycle-fixtures.mjs";
import {
  isolationRecord,
  startC4Harness,
  writeC4Fixture,
  type C4Harness,
} from "./probe-c4-support.mjs";

const SEED = { "hello.txt": "hello\n" };
const CONNECTION_ID = "conn-c4-probe";
const TEXT_ATTACHMENT = "attachment body line one\nattachment body line two\n";
// 1x1 PNG（67 字节，真实二进制）。
const PNG_ATTACHMENT = Buffer.from(
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
  "base64",
);

function sha256(bytes: Buffer): string {
  return `sha256:${createHash("sha256").update(bytes).digest("hex")}`;
}

/** 附件事务的三个调用（begin/chunk/commit），uploadId 由调用方固定（夹具可复现）。 */
async function uploadAttachment(
  harness: C4Harness,
  input: { uploadId: string; fileName: string; mime: string; bytes: Buffer },
): Promise<{ begin: JsonObject; chunks: JsonObject[]; commit: JsonObject; ref: string }> {
  const totalChunks = Math.ceil(input.bytes.byteLength / (512 * 1024)) || 1;
  const begin = await harness.tryRequest("v4/attachment/begin", {
    sessionId: harness.session.sessionId,
    uploadId: input.uploadId,
    fileName: input.fileName,
    mime: input.mime,
    totalBytes: input.bytes.byteLength,
    totalChunks,
    checksum: sha256(input.bytes),
    connectionId: CONNECTION_ID,
  });
  const chunks: JsonObject[] = [];
  for (let index = 0; index < totalChunks; index += 1) {
    const start = index * 512 * 1024;
    const slice = input.bytes.subarray(start, Math.min(start + 512 * 1024, input.bytes.byteLength));
    const chunk = await harness.tryRequest("v4/attachment/chunk", {
      sessionId: harness.session.sessionId,
      uploadId: input.uploadId,
      chunkIndex: index,
      dataBase64: slice.toString("base64"),
      connectionId: CONNECTION_ID,
    });
    chunks.push({ chunkIndex: index, ...(chunk.ok ? chunk.result : { error: chunk.error }) });
  }
  const commit = await harness.tryRequest("v4/attachment/commit", {
    sessionId: harness.session.sessionId,
    uploadId: input.uploadId,
    connectionId: CONNECTION_ID,
  });
  const commitPayload = (commit.ok ? commit.result : {}) as JsonObject;
  return {
    begin: (begin.ok ? begin.result : begin.error) as JsonObject,
    chunks,
    commit: (commit.ok ? commit.result : commit.error) as JsonObject,
    ref: typeof commitPayload.ref === "string" ? commitPayload.ref : "",
  };
}

/** v4/command sendText 信封。 */
async function sendTextCommand(
  harness: C4Harness,
  commandId: string,
  payload: JsonObject,
): Promise<JsonObject> {
  const result = await harness.tryRequest("v4/command", {
    commandId,
    clientId: "probe-c4",
    sessionId: harness.session.sessionId,
    type: "sendText",
    payload,
    issuedAt: Date.now(),
  });
  return (result.ok ? result.result : result.error) as JsonObject;
}

/** 信封的通用形状投影：键集合 + 标量/数组长度，避免把整帧正文写进夹具。 */
function describeShape(value: unknown, depth = 0): unknown {
  if (value === null || value === undefined) return null;
  if (typeof value === "string") {
    return value.length <= 120
      ? { string: value }
      : { stringHead: value.slice(0, 120), chars: value.length };
  }
  if (typeof value !== "object") return value;
  if (Array.isArray(value)) {
    return {
      arrayLength: value.length,
      item: value.length > 0 && depth < 3 ? describeShape(value[0], depth + 1) : null,
    };
  }
  const entries = Object.entries(value as JsonObject);
  const shape: JsonObject = { keys: entries.map(([key]) => key).sort() };
  if (depth < 3) {
    for (const [key, inner] of entries.slice(0, 12)) shape[key] = describeShape(inner, depth + 1);
  }
  return shape;
}

/** v4/conversation/frame 通知的信封投影（载荷只记 kind 与行 kind / delta 键）。 */
function projectFrames(notifications: JsonObject[]): JsonObject {
  const payloadKinds = new Set<string>();
  const envelopeKinds = new Set<string>();
  const deliveryKinds = new Set<string>();
  const deltaOps = new Set<string>();
  const deltaRowKinds = new Set<string>();
  const deltaKeys = new Set<string>();
  let snapshotKeys: string[] | null = null;
  let snapshotSample: unknown = null;
  for (const params of notifications) {
    const frame = (params.frame ?? params.payload ?? {}) as JsonObject;
    const payload = (frame.payload ?? frame) as JsonObject;
    payloadKinds.add(String(payload.kind ?? ""));
    envelopeKinds.add(String(params.kind ?? ""));
    deliveryKinds.add(String(params.deliveryKind ?? ""));
    const snapshot = payload.snapshot as JsonObject | undefined;
    if (snapshot !== undefined) {
      snapshotKeys ??= Object.keys(snapshot).sort();
      snapshotSample ??= describeShape(snapshot, 0);
    }
    const deltas = Array.isArray(payload.deltas) ? (payload.deltas as JsonObject[]) : [];
    for (const delta of deltas) {
      const op = String(delta.op ?? "");
      deltaOps.add(op);
      deltaKeys.add(Object.keys(delta).sort().join(","));
      const row = delta.row as JsonObject | undefined;
      if (row !== undefined) deltaRowKinds.add(String(row.kind ?? ""));
    }
  }
  return {
    payloadKinds: [...payloadKinds].sort(),
    envelopeKinds: [...envelopeKinds].sort(),
    deliveryKinds: [...deliveryKinds].sort(),
    paramsKeys: notifications.length > 0 ? Object.keys(notifications[0] ?? {}).sort() : [],
    snapshotKeys,
    snapshotSample,
    deltaOps: [...deltaOps].sort(),
    deltaRowKinds: [...deltaRowKinds].sort(),
    deltaKeys: [...deltaKeys].sort(),
  };
}

/** 等第 expected 个 turn 终态（session/event 流）。 */
async function waitForTerminal(harness: C4Harness, expected: number, what: string): Promise<void> {
  const deadline = Date.now() + 180_000;
  const terminalAt = (): number =>
    harness.captureState.events.filter(
      (event) => event.type === "turn.completed" || event.type === "turn.failed",
    ).length;
  while (terminalAt() < expected) {
    if (Date.now() > deadline) {
      const recent = harness.captureState.events.slice(-8).map((event) => ({
        type: event.type,
        payloadHead: JSON.stringify(event.payload).slice(0, 200),
      }));
      throw new Error(
        `oracle 在 180s 内没有观察到 ${what}；最近事件=${JSON.stringify(recent)}；` +
          `stderrTail=${harness.session.stderr.slice(-400)}`,
      );
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  await new Promise((resolve) => setTimeout(resolve, 500));
}

/** 找 dataRoot 下候选附件落盘文件（跳过已知非附件目录），记录相对路径。 */
async function findAttachmentFiles(dataRoot: string): Promise<JsonObject[]> {
  const results: JsonObject[] = [];
  const skip = new Set(["log", "db", "rollout", "plugins", "skills", "workflows", "c4-fixtures"]);
  const walk = async (relative: string, depth: number): Promise<void> => {
    if (depth > 5 || results.length >= 12) return;
    let entries: string[];
    try {
      entries = await readdir(join(dataRoot, relative));
    } catch {
      return;
    }
    for (const entry of entries) {
      if (results.length >= 12) return;
      const childRelative = relative ? `${relative}/${entry}` : entry;
      let isDirectory = false;
      try {
        await readdir(join(dataRoot, childRelative));
        isDirectory = true;
      } catch {
        isDirectory = false;
      }
      if (isDirectory) {
        if (relative === ".mode/cli" && skip.has(entry)) continue;
        await walk(childRelative, depth + 1);
      } else if (depth >= 2) {
        results.push({ path: childRelative });
      }
    }
  };
  await walk("", 0);
  return results;
}

async function caseAttachmentsAndFrames(): Promise<JsonObject> {
  const dataRoot = await mkdtemp(join(tmpdir(), "mode-c4-att-"));
  const cwd = await mkdtemp(join(tmpdir(), "mode-c4-att-ws-"));
  const harness = await startC4Harness({
    prefix: "c4-att",
    seed: SEED,
    dataRoot,
    cwd,
    script: [
      (() => {
        const consumed = new Set<string>();
        return (body: JsonObject): JsonObject => {
          const messages = Array.isArray(body.messages) ? (body.messages as JsonObject[]) : [];
          const texts: string[] = [];
          for (const message of messages) {
            if (message.role !== "user") continue;
            if (typeof message.content === "string") texts.push(message.content);
            else if (Array.isArray(message.content)) {
              for (const part of message.content as JsonObject[]) {
                if (typeof part.text === "string") texts.push(part.text);
              }
            }
          }
          const has = (needle: string): boolean => texts.some((text) => text.includes(needle));
          if (has("write a file") && !consumed.has("write")) {
            consumed.add("write");
            return {
              toolCalls: [
                {
                  name: "Write",
                  input: { file_path: "${cwd}/probe-written.txt", content: "written by probe\n" },
                },
              ],
            };
          }
          if (has("run background bash") && !consumed.has("bash")) {
            consumed.add("bash");
            return {
              toolCalls: [
                {
                  name: "Bash",
                  input: { command: "echo background-output-line", run_in_background: true },
                },
              ],
            };
          }
          return { text: "attachment answer" };
        };
      })(),
    ],
  });
  try {
    // Phase A：先订阅（帧覆盖后续全部动作）。
    const subscribe = await harness.tryRequest("v4/conversation/subscribe", {
      topic: `conversation/${harness.session.sessionId}`,
      connectionId: CONNECTION_ID,
      clientMode: "desktop-continuous",
    });
    const ackEnvelope = (subscribe.ok ? subscribe.result : subscribe.error) as JsonObject;
    const ack = (ackEnvelope.ack ?? {}) as JsonObject;
    // extras 的键是「要折叠的原始值」，值是替换后的稳定 token。
    if (typeof ack.logEpoch === "string") harness.extras[ack.logEpoch] = "<log-epoch>";
    if (typeof ack.subscriptionId === "string") {
      harness.extras[ack.subscriptionId] = "<subscription>";
    }
    const framesAfterSubscribe = harness.notifications("v4/conversation/frame").length;

    // Phase B：文本附件全事务。
    const textUpload = await uploadAttachment(harness, {
      uploadId: "upload-c4-text-1",
      fileName: "note.txt",
      mime: "text/plain",
      bytes: Buffer.from(TEXT_ATTACHMENT, "utf8"),
    });

    // Phase C：v4 sendText 带 attachments（实测 FK 失败——oracle 缺陷登记）。
    const sendTextAck = await sendTextCommand(harness, "cmd-c4-att-text-1", {
      text: "please look at the attached note",
      attachments: [
        {
          ref: textUpload.ref,
          fileName: "note.txt",
          mime: "text/plain",
          bytes: Buffer.from(TEXT_ATTACHMENT, "utf8").byteLength,
        },
      ],
    });
    // 旧 session/send 带 attachments record（accepted，但 ref 不进 userInput 行）。
    let legacySendAck: JsonObject;
    try {
      legacySendAck = (await harness.session.request("session/send", {
        sessionId: harness.session.sessionId,
        content: "look at the attached note via legacy send",
        attachments: [{ ref: textUpload.ref }],
      })) as JsonObject;
      await waitForTerminal(harness, 1, "legacy attachment send turn");
    } catch (error) {
      legacySendAck = { thrown: String(error).slice(0, 300) };
    }

    // Phase D：读面对 committed ref 的授权形状（全部带真实参数形状）。
    const rowsRangeAll = await harness.tryRequest("v4/conversation/rowsRange", {
      sessionId: harness.session.sessionId,
      limit: 20,
    });
    const rows =
      (((rowsRangeAll.result as JsonObject | undefined)?.rows ?? []) as JsonObject[]) || [];
    const userInputRow = rows.find((row) => row.kind === "userInput");
    const target = {
      rowId: Number(userInputRow?.rowId ?? 0),
      entityId: String(userInputRow?.entityId ?? "missing-entity"),
    };
    const attachmentRead = await harness.tryRequest("v4/attachment/read", {
      sessionId: harness.session.sessionId,
      ref: textUpload.ref,
      offset: 0,
      limit: 65536,
    });
    const previewSource = await harness.tryRequest("v4/attachment/previewSource", {
      sessionId: harness.session.sessionId,
      ref: textUpload.ref,
      clientMode: "desktop-continuous",
    });
    const attachmentStat = await harness.tryRequest("v4/conversation/attachmentStat", {
      sessionId: harness.session.sessionId,
      ref: textUpload.ref,
      target,
      attachmentIndex: 0,
    });
    const conversationAttachmentRead = await harness.tryRequest("v4/conversation/attachmentRead", {
      sessionId: harness.session.sessionId,
      ref: textUpload.ref,
      target,
      attachmentIndex: 0,
      offset: 0,
      limit: 65536,
    });
    const legacyMessages = (await harness.session.request("session/messages", {
      sessionId: harness.session.sessionId,
    })) as JsonObject;
    const legacyParts = ((legacyMessages.messages ?? []) as JsonObject[]).map((message) => ({
      role: (message.info as JsonObject | undefined)?.role ?? null,
      parts: ((message.parts ?? []) as JsonObject[]).map((part) => part.type ?? null),
    }));

    // Phase E：二进制附件（PNG）全事务 + 读面尝试。
    const pngUpload = await uploadAttachment(harness, {
      uploadId: "upload-c4-png-1",
      fileName: "probe.png",
      mime: "image/png",
      bytes: PNG_ATTACHMENT,
    });
    const pngRead = await harness.tryRequest("v4/attachment/read", {
      sessionId: harness.session.sessionId,
      ref: pngUpload.ref,
      offset: 0,
      limit: 65536,
    });
    const pngPreviewSource = await harness.tryRequest("v4/attachment/previewSource", {
      sessionId: harness.session.sessionId,
      ref: pngUpload.ref,
      clientMode: "desktop-continuous",
    });

    // Phase F：abort 事务 + abort 后 commit 的错误形状。
    const beginForAbort = await harness.tryRequest("v4/attachment/begin", {
      sessionId: harness.session.sessionId,
      uploadId: "upload-c4-abort-1",
      fileName: "aborted.txt",
      mime: "text/plain",
      totalBytes: 10,
      totalChunks: 1,
      checksum: sha256(Buffer.from("0123456789", "utf8")),
      connectionId: CONNECTION_ID,
    });
    const abort = await harness.tryRequest("v4/attachment/abort", {
      sessionId: harness.session.sessionId,
      uploadId: "upload-c4-abort-1",
      connectionId: CONNECTION_ID,
    });
    const commitAfterAbort = await harness.tryRequest("v4/attachment/commit", {
      sessionId: harness.session.sessionId,
      uploadId: "upload-c4-abort-1",
      connectionId: CONNECTION_ID,
    });

    // Phase G：只读查询族（Write turn → fileChanges；后台 bash turn → backgroundBashOutput）。
    await harness.session.send("please write a file");
    await waitForTerminal(harness, 2, "write turn");
    const rowsAfterWrite = await harness.tryRequest("v4/conversation/rowsRange", {
      sessionId: harness.session.sessionId,
      limit: 50,
    });
    const rows2 =
      (((rowsAfterWrite.result as JsonObject | undefined)?.rows ?? []) as JsonObject[]) || [];
    const turnHeader = rows2.find(
      (row) => row.kind === "turnHeader" && row.state === "completedSuccess",
    );
    const fileChanges = await harness.tryRequest("v4/conversation/fileChanges", {
      sessionId: harness.session.sessionId,
      target: {
        rowId: Number(turnHeader?.rowId ?? 0),
        entityId: String(turnHeader?.entityId ?? "missing-entity"),
      },
      baseRevision: Number((rowsAfterWrite.result as JsonObject | undefined)?.atRevision ?? 0),
      baseLogEpoch: String(
        (rowsAfterWrite.result as JsonObject | undefined)?.atLogEpoch ?? "epoch",
      ),
    });
    await harness.session.send("run background bash please");
    await waitForTerminal(harness, 3, "background bash turn");
    const workId = (() => {
      for (const event of harness.captureState.events) {
        const payload = (event.payload ?? {}) as JsonObject;
        if (event.type === "tool.updated" && payload.kind === "result") {
          const match = /workId:\s*([^\s"]+)/u.exec(
            String((payload.result as JsonObject)?.content ?? ""),
          );
          if (match) return match[1];
        }
      }
      return "";
    })();
    const backgroundBash = workId
      ? await harness.tryRequest("v4/conversation/backgroundBashOutput", {
          sessionId: harness.session.sessionId,
          workId,
        })
      : ({ ok: false, error: { note: "no workId observed" } } as JsonObject);
    const plans = await harness.tryRequest("v4/conversation/plans", {
      sessionId: harness.session.sessionId,
    });
    const usage = await harness.tryRequest("v4/conversation/usage", {
      sessionId: harness.session.sessionId,
    });
    const usageStats = await harness.tryRequest("v4/usage/stats", { range: "7d" });
    const commandsQuery = await harness.tryRequest("v4/commands/query", {
      commands: [{ sessionId: harness.session.sessionId, commandId: "cmd-c4-att-text-1" }],
    });
    // rowsRange 分页：beforeRowId = 尾行 rowId。
    const tailRowId = Number(rows2.at(-1)?.rowId ?? 0);
    const rowsRangePage = await harness.tryRequest("v4/conversation/rowsRange", {
      sessionId: harness.session.sessionId,
      beforeRowId: tailRowId,
      limit: 2,
    });

    // Phase H：resync / sessions-index 尝试 / unsubscribe。
    const resync = await harness.tryRequest("v4/conversation/resync", {
      topic: `conversation/${harness.session.sessionId}`,
      connectionId: CONNECTION_ID,
      subscriptionId: ack.subscriptionId ?? "missing-subscription",
      base: { logEpoch: ack.logEpoch ?? "epoch", seq: 0 },
    });
    const sessionsIndexSubscribe = await harness.tryRequest("v4/conversation/subscribe", {
      topic: `sessions-index/${cwd}`,
      connectionId: CONNECTION_ID,
      clientMode: "desktop-continuous",
    });
    const unsubscribe = await harness.tryRequest("v4/conversation/unsubscribe", {
      topic: `conversation/${harness.session.sessionId}`,
      connectionId: CONNECTION_ID,
      subscriptionId: ack.subscriptionId ?? "missing-subscription",
    });
    const framesAfterUnsubscribe = harness.notifications("v4/conversation/frame").length;
    await harness.session.send("one more prompt after unsubscribe");
    await waitForTerminal(harness, 4, "post-unsubscribe turn");
    const framesAtEnd = harness.notifications("v4/conversation/frame").length;
    // 计数只进 stdout 摘要，不进夹具（合并时机不同会差 1 帧）。
    process.stdout.write(
      `[probe-c4-attachments] frames afterSubscribe=${framesAfterSubscribe} ` +
        `afterUnsubscribe=${framesAfterUnsubscribe} atEnd=${framesAtEnd}\n`,
    );

    // Phase I：DB 行与落盘。
    const tables = harness.readC4Tables(["session_input", "input_history"]);
    const attachmentFiles = (await findAttachmentFiles(dataRoot)).map((entry) => {
      // 路径里同时有会话 id、上传 id（含 logEpoch 前缀）与 shell 快照时间戳：
      // 统一折叠，夹具只保留布局与命名规则。
      const path = String(harness.normalizeForFixture(entry.path));
      return {
        path: path
          .replace(/sess_<id>/gu, "<session-id>")
          .replace(
            /prompt-attachment-upload-[a-z0-9]+-[a-z0-9]+-tool-result-<id>/gu,
            "prompt-attachment-upload-<upload>-tool-result-<id>",
          )
          .replace(/snapshot-bash-\d+-[a-z0-9]+\.sh/gu, "snapshot-bash-<time>-<id>.sh")
          .replace(/image-[0-9a-f]+\.png/gu, "image-<id>.png"),
      };
    });
    // sessions-index 订阅会生成新的 logEpoch/subscriptionId；先登记再重新规范化。
    const sessionsIndexAck = sessionsIndexSubscribe.ok
      ? ((sessionsIndexSubscribe.result.ack ?? {}) as JsonObject)
      : {};
    if (typeof sessionsIndexAck.subscriptionId === "string") {
      harness.extras[sessionsIndexAck.subscriptionId] = "<sessions-index-subscription>";
    }
    if (typeof sessionsIndexAck.logEpoch === "string") {
      harness.extras[sessionsIndexAck.logEpoch] = "<sessions-index-log-epoch>";
    }
    const sessionsIndexFolded = harness.normalizeForFixture(sessionsIndexSubscribe) as JsonObject;
    // usageStats 的 heatmap 起止日期随运行日滚动；登记后重新规范化。
    const heatmap = usageStats.ok ? ((usageStats.result.heatmap ?? {}) as JsonObject) : {};
    if (typeof heatmap.startDate === "string") harness.extras[heatmap.startDate] = "<range-start>";
    if (typeof heatmap.endDate === "string") harness.extras[heatmap.endDate] = "<range-end>";
    const usageStatsFolded = usageStats.ok
      ? ({ ok: true, result: harness.normalizeForFixture(usageStats.result) } as JsonObject)
      : usageStats;

    return {
      isolation: isolationRecord(harness),
      subscribeAck: harness.normalizeForFixture({ ok: subscribe.ok, ack }) as JsonObject,
      frames: {
        // 帧数量随合并时机变化（20/21 都观察到），只记形状集合，不把计数写进夹具。
        afterSubscribe: "<count>",
        projection: projectFrames(harness.notifications("v4/conversation/frame")),
        afterUnsubscribe: "<count>",
        atEnd: "<count>",
      },
      textUpload,
      sendTextAck,
      legacySendAck: harness.normalizeForFixture(legacySendAck) as JsonObject,
      rowsRange: {
        atSeq: (rowsRangeAll.result as JsonObject | undefined)?.atSeq ?? null,
        atRevision: (rowsRangeAll.result as JsonObject | undefined)?.atRevision ?? null,
        rowKinds: rows.map((row) => row.kind),
        userInputAttachments: userInputRow?.attachments ?? null,
        page: rowsRangePage,
      },
      readFace: {
        attachmentRead,
        previewSource,
        attachmentStat,
        conversationAttachmentRead,
        legacyParts,
      },
      pngUpload,
      pngReadFace: { attachmentRead: pngRead, previewSource: pngPreviewSource },
      abortTransaction: {
        begin: beginForAbort.ok ? beginForAbort.result : beginForAbort.error,
        abort: abort.ok ? abort.result : abort.error,
        commitAfterAbort: commitAfterAbort.ok ? commitAfterAbort.result : commitAfterAbort.error,
      },
      queries: {
        fileChanges,
        backgroundBash,
        plans,
        usage,
        usageStats: usageStatsFolded,
        commandsQuery,
      },
      resync,
      sessionsIndexSubscribe: sessionsIndexFolded,
      unsubscribe,
      dbTables: tables,
      attachmentFiles,
    };
  } finally {
    await harness.stop();
  }
}

const fixtures: Record<string, JsonObject> = {
  "attachments-v4-frames": {
    case: "attachments-v4-frames",
    note:
      "C4 Task 0 oracle 捕获：v4/attachment begin/chunk/commit/abort 全事务（text+PNG 二进制，" +
      "ref = mode-artifact://<session>/tool-result-<id>）；v4/command sendText 带 attachments 的 " +
      "FK 失败（oracle 缺陷登记）；旧 session/send 的 attachments record 形状（accepted 但 ref 不进 " +
      "userInput 行）；attachmentRead / conversationAttachmentStat/Read / previewSource 的授权错误形状；" +
      "v4/conversation/subscribe 帧（快照/增量）、resync/unsubscribe、sessions-index 订阅尝试；" +
      "rowsRange 分页/plans/usage/fileChanges/backgroundBashOutput/usageStats/commandsQuery；" +
      "session_input DB 行。",
    input: {
      seed: SEED,
      connectionId: CONNECTION_ID,
      attachments: [
        { uploadId: "upload-c4-text-1", fileName: "note.txt", mime: "text/plain" },
        { uploadId: "upload-c4-png-1", fileName: "probe.png", mime: "image/png", bytes: "1x1 png" },
        { uploadId: "upload-c4-abort-1", fileName: "aborted.txt", aborted: true },
      ],
      prompts: [
        "look at the attached note via legacy send",
        "please write a file",
        "run background bash please",
        "one more prompt after unsubscribe",
      ],
    },
    expect: await caseAttachmentsAndFrames(),
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
          const textUpload = (expect.textUpload as JsonObject | undefined) ?? {};
          return [
            name,
            {
              textCommit: textUpload.commit ?? null,
              sendTextAck: (expect.sendTextAck as JsonObject | undefined) ?? null,
              frameKinds:
                ((expect.frames as JsonObject | undefined)?.projection as JsonObject | undefined)
                  ?.payloadKinds ?? null,
            },
          ];
        }),
      ),
    },
    null,
    2,
  )}\n`,
);
