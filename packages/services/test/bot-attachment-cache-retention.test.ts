import assert from "node:assert/strict";
import test from "node:test";
import { mkdtemp, mkdir, readdir, rm, utimes, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

// bot 附件缓存的保留期与清理（docs/specs/bot-attachment-cache-retention.md）：
// 缓存只增不减，且被存储清理按 config 类保护掉（CLEANABILITY.config = none），
// 所以补自动保留期 + 总量上限。这里钉住计划函数的三条规则与两条安全约束。

const DAY = 24 * 60 * 60 * 1000;
const NOW = 1_000 * DAY; // 固定 now，避免测试随真实时间漂移

async function makeTempRoot(): Promise<string> {
  return mkdtemp(join(tmpdir(), "bot-attachment-retention-"));
}

/** 造一个 mtime 为 ageMs 之前的缓存文件，返回其绝对路径。 */
async function writeCacheFile(root: string, relative: string, ageMs: number): Promise<string> {
  const full = join(root, relative);
  await mkdir(dirname(full), { recursive: true });
  await writeFile(full, "x");
  const mtime = new Date(Date.now() - ageMs);
  await utimes(full, mtime, mtime);
  return full;
}

test("R1：超过保留期的非受保护文件被选中，未过期的不动", async () => {
  const { planAttachmentCacheEviction } = await import("../src/bots/attachmentCacheRetention.js");
  const stale = { path: "/root/a/old.png", sizeBytes: 10, mtimeMs: NOW - 8 * DAY };
  const fresh = { path: "/root/a/new.png", sizeBytes: 10, mtimeMs: NOW - 1 * DAY };
  const evicted = planAttachmentCacheEviction({
    entries: [stale, fresh],
    protectedPaths: new Set(),
    now: NOW,
    maxAgeMs: 7 * DAY,
  });
  assert.deepEqual(
    evicted.map((entry) => entry.path),
    [stale.path],
  );
});

test("R3：受保护文件即使早已过期也不删", async () => {
  const { planAttachmentCacheEviction } = await import("../src/bots/attachmentCacheRetention.js");
  const queued = { path: "/root/a/queued.png", sizeBytes: 10, mtimeMs: NOW - 30 * DAY };
  const evicted = planAttachmentCacheEviction({
    entries: [queued],
    protectedPaths: new Set([queued.path]),
    now: NOW,
    maxAgeMs: 7 * DAY,
  });
  assert.equal(evicted.length, 0, "队列仍在引用的文件不能被年龄规则删掉");
});

test("R3：受保护集合比较做分隔符归一（Windows 反斜杠不会漏判）", async () => {
  const { planAttachmentCacheEviction } = await import("../src/bots/attachmentCacheRetention.js");
  const entry = { path: "C:/root/bot-attachments/b1/m1/x.png", sizeBytes: 1, mtimeMs: NOW - 30 * DAY };
  const evicted = planAttachmentCacheEviction({
    entries: [entry],
    protectedPaths: new Set(["C:\\root\\bot-attachments\\b1\\m1\\x.png"]),
    now: NOW,
    maxAgeMs: 7 * DAY,
  });
  assert.equal(evicted.length, 0, "分隔符不同不能导致受保护文件被误删");
});

test("R2：总量超上限时按 mtime 由旧到新淘汰", async () => {
  const { planAttachmentCacheEviction } = await import("../src/bots/attachmentCacheRetention.js");
  const entries = [
    { path: "/root/a/1.png", sizeBytes: 100, mtimeMs: NOW - 3 * DAY },
    { path: "/root/a/2.png", sizeBytes: 100, mtimeMs: NOW - 2 * DAY },
    { path: "/root/a/3.png", sizeBytes: 100, mtimeMs: NOW - 1 * DAY },
  ];
  const evicted = planAttachmentCacheEviction({
    entries,
    protectedPaths: new Set(),
    now: NOW,
    maxAgeMs: 3650 * DAY,
    maxTotalBytes: 250,
  });
  assert.deepEqual(
    evicted.map((entry) => entry.path),
    ["/root/a/1.png"],
    "300 > 250，只淘汰最旧的一个即回到 200",
  );
});

test("R2：受保护文件不占配额（否则会挤掉更新、可删的文件）", async () => {
  const { planAttachmentCacheEviction } = await import("../src/bots/attachmentCacheRetention.js");
  const queued = { path: "/root/a/queued.png", sizeBytes: 10_000, mtimeMs: NOW - 1 * DAY };
  const entries = [
    { path: "/root/a/1.png", sizeBytes: 100, mtimeMs: NOW - 3 * DAY },
    { path: "/root/a/2.png", sizeBytes: 100, mtimeMs: NOW - 2 * DAY },
  ];
  const evicted = planAttachmentCacheEviction({
    entries: [...entries, queued],
    protectedPaths: new Set([queued.path]),
    now: NOW,
    maxAgeMs: 3650 * DAY,
    maxTotalBytes: 250,
  });
  assert.equal(evicted.length, 0, "可删文件合计 200 未超 250，受保护的大文件不该参与计算");
});

test("执行清理：删除过期文件并回收随之变空的目录", async () => {
  const { pruneAttachmentCache } = await import("../src/bots/attachmentCacheRetention.js");
  const root = await makeTempRoot();
  try {
    await writeCacheFile(root, "bot1/msg-old/old.png", 8 * DAY);
    await writeCacheFile(root, "bot1/msg-new/new.png", 1 * DAY);
    const result = await pruneAttachmentCache({ rootDir: root, protectedPaths: new Set() });
    assert.equal(result.deletedCount, 1);
    assert.ok(result.freedBytes > 0);
    assert.equal(result.failedCount, 0);
    const bots = await readdir(root);
    assert.deepEqual(bots, ["bot1"], "过期文件所在目录应被回收");
    assert.deepEqual(await readdir(join(root, "bot1")), ["msg-new"], "新文件保留");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("执行清理：队列引用的文件保留，同批其他过期文件照删", async () => {
  const { pruneAttachmentCache } = await import("../src/bots/attachmentCacheRetention.js");
  const root = await makeTempRoot();
  try {
    const queuedPath = await writeCacheFile(root, "bot1/msg-q/queued.png", 30 * DAY);
    await writeCacheFile(root, "bot1/msg-x/other.png", 30 * DAY);
    const result = await pruneAttachmentCache({
      rootDir: root,
      protectedPaths: new Set([queuedPath]),
    });
    assert.equal(result.deletedCount, 1);
    assert.deepEqual(await readdir(join(root, "bot1")), ["msg-q"], "只有非受保护的那个目录该消失");
    assert.deepEqual(
      await readdir(join(root, "bot1", "msg-q")),
      ["queued.png"],
      "受保护文件必须还在",
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("执行清理：根目录不存在不抛错（后台维护不能带崩入站）", async () => {
  const { pruneAttachmentCache } = await import("../src/bots/attachmentCacheRetention.js");
  const root = await makeTempRoot();
  try {
    const result = await pruneAttachmentCache({
      rootDir: join(root, "not-created"),
      protectedPaths: new Set(),
    });
    assert.deepEqual(result, { deletedCount: 0, freedBytes: 0, failedCount: 0 });
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

const botsServiceSource = await import("node:fs/promises").then((fs) =>
  fs.readFile(new URL("../src/bots/botsService.ts", import.meta.url), "utf8").catch(() => null),
);

/** 取 `start` 标记之后的窗口（行窗口，避免跨函数误命中）。 */
function windowAfter(source: string, marker: string, lineSpan: number): string {
  const index = source.indexOf(marker);
  assert.ok(index >= 0, `未找到锚点：${marker}`);
  return source.slice(index).split("\n").slice(0, lineSpan).join("\n");
}

test("接线：清理挂在唯一入站入口，且受保护集合取自队列", () => {
  assert.ok(botsServiceSource, "无法读取 botsService 源码");
  const source = botsServiceSource!;
  const entry = windowAfter(source, "async function processProviderCallback", 20);
  assert.ok(
    entry.includes("pruneAttachmentCacheIfDue()"),
    "清理必须挂在入站入口——没有别的触发点",
  );
  const collector = windowAfter(source, "function collectQueuedAttachmentPaths", 16);
  assert.ok(
    collector.includes("queuedMessages") && collector.includes("localPath"),
    "受保护集合必须来自队列附件的 localPath",
  );
});

test("R5：localPath 读不到时返回 null，不把整条排队消息带走", () => {
  assert.ok(botsServiceSource, "无法读取 botsService 源码");
  const resolver = windowAfter(botsServiceSource!, "async function resolveAttachmentBytes", 30);
  assert.ok(
    resolver.includes('"ENOENT"'),
    "必须只把 ENOENT 降级为解析失败，其他读错误仍应暴露",
  );
  assert.ok(
    resolver.includes("return null"),
    "降级路径要返回 null，走既有「未能下载」分支",
  );
});
