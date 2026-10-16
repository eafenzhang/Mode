// M2 Task 2 探针：**`offset` 越界提醒**与两道上限的先后（诊断用，不进对拍套件）。
//
// 用法：
//   pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-read-warnings.mts
//
// 为什么单独一个文件：`probe-read.mts` 加到这个组之后是 396 代码行（oxlint 的 `max-lines` 是 400），
// 只差 4 行就要把 `pnpm lint` 变成错误。这组是一个独立关注点（越界提醒的判据 + 它和上限的先后），
// 拆开后两个文件都有余量。
//
// 结论（每组探测的原始输出见运行日志；规则写进 `crates/agent-tools/src/read.rs` 的文件头）：
//
// - 提醒**先于**两道上限（`w-big-overbudget-offset-2`：超预算单行文件 + `offset: 2` 给提醒）；
// - 判据是 `offset > **真实行数**`，真实行数**不含末尾空段**（`w-twoline-offset-2` 给正文 vs
//   `w-threeline-offset-3` 给提醒，两文件的段数分别是 2 与 3、offset 分别是 2 与 3）；
// - 空文件真实行数是 0，且 `offset > 1` 走**另一条**文案（`w-empty-offset-2`）；
// - **`offset: 0` 走隐式路径**（`w-big-overbudget-offset-0` 给部分视图），所以它不是「显式范围」。
import { runGroup, type ProbeGroup } from "./probe-support.mjs";

/**
 * **`offset` 越界提醒与 token 预算谁先谁后**（复评 fix round 2 的核心问题）。
 *
 * 现有观察里有一处**自相矛盾**：空文件（1 段、offset 缺省 1）给的是越界提醒，而**单行**的大文件
 * （同样 1 段、offset 缺省 1）给的却是正文/部分视图。所以判据不是单纯的 `offset >= 段数`。
 * 本组把「行数、有没有结尾换行、offset 是多少」三个维度分开问：
 *
 * | 探测 | 段数 | 结尾换行 | offset | 想区分什么 |
 * | --- | --- | --- | --- | --- |
 * | `w-empty-offset-1` | 1（空文件） | — | 缺省 1 | 空文件的那条提醒（已知） |
 * | `w-empty-offset-2` | 1（空文件） | — | 2 | 空文件 + 越界 offset |
 * | `w-single60000-default` | 1 | 无 | 缺省 1 | **单行非空 + 缺省 offset 不该提醒**（已知） |
 * | `w-single60000-offset-1` | 1 | 无 | 显式 1 | 显式 1 与缺省 1 是否同路 |
 * | `w-single60000-offset-2` | 1 | 无 | 2 | 单行文件上 offset 2 算不算越界 |
 * | `w-twoline-offset-2` | 2（`a`/`b`） | 无 | 2 | 段数 2 + offset 2 |
 * | `w-threeline-offset-3` | 3（`a`/`b`/空） | **有** | 3 | 段数 3（含末尾空段）+ offset 3 |
 * | `w-threeline-offset-4` | 3 | **有** | 4 | 同上、再越一格 |
 * | `w-fourline-offset-3` | 4 | **有** | 3 | 已知给正文（对照） |
 * | `w-big-offset-0` | 1 | 无 | 0 | 超预算文件 + offset 0：走部分视图还是错误（「显式」判据） |
 * | `w-big-offset-2` | 1 | 无 | 2 | 超预算文件 + offset 2：错误（对照） |
 */
function offsetWarningGroup(): ProbeGroup {
  // 每个探测**各自一个文件**：同名文件第二次读会命中「重复读去重」（第一版探针在这里翻过车：
  // `offset: 1` 那次拿到的是上一个探测留下的「Wasted call」填充语）。
  const single = (name: string) => ({ [name]: "q".repeat(60_000) });
  const big = (name: string, width: number) => ({ [name]: "q".repeat(width) });
  const probes: Probe[] = [
    { label: "w-empty-offset-1", seed: { "e1.txt": "" }, args: { file_path: "${cwd}/e1.txt" } },
    {
      label: "w-empty-offset-2",
      seed: { "e2.txt": "" },
      args: { file_path: "${cwd}/e2.txt", offset: 2 },
    },
    {
      label: "w-single60000-default",
      seed: single("s-default.txt"),
      args: { file_path: "${cwd}/s-default.txt" },
    },
    {
      label: "w-single60000-offset-1",
      seed: single("s-offset1.txt"),
      args: { file_path: "${cwd}/s-offset1.txt", offset: 1 },
    },
    {
      label: "w-single60000-offset-2",
      seed: single("s-offset2.txt"),
      args: { file_path: "${cwd}/s-offset2.txt", offset: 2 },
    },
    {
      label: "w-twoline-offset-2",
      seed: { "t2.txt": "a\nb" },
      args: { file_path: "${cwd}/t2.txt", offset: 2 },
    },
    {
      label: "w-threeline-offset-3",
      seed: { "t3.txt": "a\nb\n" },
      args: { file_path: "${cwd}/t3.txt", offset: 3 },
    },
    {
      label: "w-threeline-offset-4",
      seed: { "t4.txt": "a\nb\n" },
      args: { file_path: "${cwd}/t4.txt", offset: 4 },
    },
    {
      label: "w-fourline-offset-3",
      seed: { "t5.txt": "1\n2\n3\n" },
      args: { file_path: "${cwd}/t5.txt", offset: 3 },
    },
    {
      label: "w-big-overbudget-offset-0",
      seed: big("b0.txt", 75_001),
      args: { file_path: "${cwd}/b0.txt", offset: 0 },
    },
    {
      label: "w-big-overbudget-offset-2",
      seed: big("b2.txt", 75_001),
      args: { file_path: "${cwd}/b2.txt", offset: 2 },
    },
  ];
  return { tool: "Read", name: "offset-warning", probes };
}

const probes = await runGroup(offsetWarningGroup());
process.stdout.write(`
##### offset-warning: ${probes} 个探针 #####
`);
