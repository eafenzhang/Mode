// M2 Task 3 探针：`Edit` 的行为面（诊断用，不进对拍套件）。
//
// 用法：
//   pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-edit.mts
//
// 纪律（docs/specs/clean-room-policy.md §黑盒对拍）：只运行旧实现、观察它的输入输出，
// 不读实现体。**期望值不在这里写死**——本文件的输出是「观察」，夹具由 `record.mts` 另录。
//
// 计划里列的、必须探到的（子计划 Task B 的 Step B1）：
//   替换必须唯一 / `old_string` 不存在 / `new_string == old_string` / `replace_all` 的语义 /
//   成功文案 / 未读就编辑 / 多行 `old_string` 的匹配口径。
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { seedWorkspace, startOracleRuntime } from "./driver.mjs";
import { describeOutcome, drain, rawToolOutcome, type MockStep } from "./probe-support.mjs";

type EditProbe = {
  label: string;
  seed: string;
  args: Record<string, unknown>;
  /** 不先 `Read`——用来单独观察「未读就编辑」那条前置条件。 */
  omitRead?: boolean;
  /** 覆盖文件相对名（默认 `e<下标>.txt`）。用来做「文件根本不存在」的探测。 */
  file?: string;
  /** 不落种子文件（配合 `file` 探不存在的路径）。 */
  skipSeed?: boolean;
};

const PROBES: EditProbe[] = [
  { label: "unique", seed: "alpha\nBeta\ngamma\n", args: { old_string: "Beta", new_string: "BETA" } },
  { label: "not-found", seed: "alpha\nBeta\n", args: { old_string: "Delta", new_string: "D" } },
  { label: "ambiguous", seed: "x\nfoo\ny\nfoo\n", args: { old_string: "foo", new_string: "bar" } },
  {
    label: "replace-all",
    seed: "x\nfoo\ny\nfoo\n",
    args: { old_string: "foo", new_string: "bar", replace_all: true },
  },
  { label: "same-string", seed: "alpha\n", args: { old_string: "alpha", new_string: "alpha" } },
  { label: "multiline", seed: "a\nb\nc\n", args: { old_string: "a\nb", new_string: "a\nX" } },
  {
    label: "indent-sensitive",
    seed: "  indented\nnot indented\n",
    args: { old_string: "indented", new_string: "INDENTED" },
  },
  {
    label: "no-read",
    seed: "gamma\n",
    args: { old_string: "gamma", new_string: "G" },
    omitRead: true,
  },
  {
    label: "read-then-delete-dir",
    seed: "delta\n",
    args: { old_string: "delta\n", new_string: "" },
  },
  // ---- 判据**先后**的探测：上面几组各自只触发一条错误，「哪条先」必须单独探，否则实现只能猜。
  {
    label: "order-same-string-not-found",
    seed: "alpha\n",
    args: { old_string: "zzz", new_string: "zzz" },
  },
  {
    label: "order-no-read-same-string",
    seed: "eps\n",
    args: { old_string: "eps", new_string: "eps" },
    omitRead: true,
  },
  // replace_all 只命中一次时，成功文案是「All occurrences」那条还是普通那条？
  {
    label: "replace-all-single-hit",
    seed: "x\nfoo\n",
    args: { old_string: "foo", new_string: "bar", replace_all: true },
  },
  // 空 old_string：子串出现次数是「长度+1」，看它走哪条分支。
  { label: "empty-old-string", seed: "zeta\n", args: { old_string: "", new_string: "Z" } },
  // 「未读」与「串不存在」谁是先判？上面两组都只触发一条，得让两条同时成立才看得出来。
  {
    label: "order-no-read-not-found",
    seed: "eta\n",
    args: { old_string: "nope", new_string: "N" },
    omitRead: true,
  },
  // 空 old_string 的语义像是「新建文件」——那么**文件不存在且没读过**时它会不会真的建出来？
  {
    label: "empty-old-nonexistent",
    args: { old_string: "", new_string: "created by edit\n" },
    omitRead: true,
    seed: "",
  },
  // 真正的「文件不存在」：`Write` 在这种情况下**不需要**先读（`write-new` 实测），
  // `Edit` 是不是也这样？这两条把写/编辑的差别钉死。
  {
    label: "missing-file-nonempty-old",
    seed: "",
    file: "never-existed.txt",
    skipSeed: true,
    omitRead: true,
    args: { old_string: "whatever", new_string: "x" },
  },
  {
    label: "missing-file-empty-old",
    seed: "",
    file: "never-existed2.txt",
    skipSeed: true,
    omitRead: true,
    args: { old_string: "", new_string: "created\n" },
  },
  // 「old == new」与「文件不存在」谁是先判？
  {
    label: "order-same-string-missing-file",
    seed: "",
    file: "never-existed3.txt",
    skipSeed: true,
    omitRead: true,
    args: { old_string: "zzz", new_string: "zzz" },
  },
];

async function main(): Promise<void> {
  const dataRoot = await mkdtemp(join(tmpdir(), "mode-probe-"));
  const cwd = await mkdtemp(join(tmpdir(), "mode-probe-ws-"));
  try {
    // 每个探针一个**独立文件**：这样可以在一个会话里跑完，又不会让 `Read` 的去重串味。
    const seed: Record<string, string> = {};
    const fileName = (probe: EditProbe, index: number): string => probe.file ?? `e${index}.txt`;
    PROBES.forEach((probe, index) => {
      if (probe.skipSeed) return;
      seed[fileName(probe, index)] = probe.seed;
    });
    await seedWorkspace(cwd, seed);

    const script: MockStep[] = [];
    const editIndexes: number[] = [];
    PROBES.forEach((probe, index) => {
      const file = `${cwd}/${fileName(probe, index)}`;
      if (!probe.omitRead) script.push({ toolCalls: [{ name: "Read", input: { file_path: file } }] });
      editIndexes.push(script.length);
      script.push({
        toolCalls: [{ name: "Edit", input: { file_path: file, ...probe.args } }],
      });
    });
    script.push({ text: "edit probe done" });

    const session = await startOracleRuntime({ dataRoot, cwd, script });
    try {
      await session.send("edit probe");
      await drain(session.events);
      process.stdout.write(`\n########## edit 探针（cwd=${cwd}）##########\n`);
      for (const [index, probe] of PROBES.entries()) {
        const observed = rawToolOutcome(session.frames, editIndexes[index] ?? 0);
        process.stdout.write(`\n== ${index}: ${probe.label} ==\n`);
        process.stdout.write(`seed: ${JSON.stringify(probe.seed)}\n`);
        process.stdout.write(`args: ${JSON.stringify(probe.args)}\n`);
        if (!observed) {
          process.stdout.write("(未观察到结果)\n");
        } else {
          process.stdout.write(`ok: ${observed.ok}\n`);
          process.stdout.write(`${describeOutcome(observed)}\n`);
        }
        const after = await readFile(join(cwd, fileName(probe, index)), "utf8").catch(() => "(不存在)");
        process.stdout.write(`${fileName(probe, index)} 现在：${JSON.stringify(after)}\n`);
      }
    } finally {
      await session.stop();
    }
  } finally {
    await rm(dataRoot, { recursive: true, force: true });
    await rm(cwd, { recursive: true, force: true });
  }
}

await main();
