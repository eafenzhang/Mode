// M2 Task 2 探针：`Read` 的**入参语义与两道上限**（诊断用，不进对拍套件）。
//
// 用法：
//   pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-read.mts [read|trunc|limit-edge|partial]
//
// ## 结论（2026-10-16 实测，原始输出见运行日志）
//
// 1. **行与行号**：正文 = `"{number}` + 制表符 + `{text}"` 用换行连接，`number = offset + 返回序号`
//    （`offset` 缺省 1）；按换行切段并**保留末尾空段**；每段去掉**行尾一个**回车、行内回车保留。
//    `offset: 0` 是一条怪癖：从 **0** 开始编号、且从第 1 行取起（起点 = `max(offset-1, 0)`）。
//    `limit` 是**行数**不是结束行号。相对 `file_path` 按会话工作目录解析（声明说必须绝对路径，实测不然）。
// 2. **`offset` 越过最后一行 → 成功**，正文是一条 system-reminder
//    （`…shorter than the provided offset ({offset}). The file has {行数} lines.…`，判据是
//    `offset >= 行数`，行数含末尾空段：空文件算 1 行）。
// 3. **截断 = token 预算**：估算 `ceil(字节数 / 3)`（74999 → 全文；75000 → 全文；75001 → 25001 超限，
//    所以是上取整且严格大于才触发），上限 25000；作用在**被选中的范围**上（源文本字节数），
//    不是整个文件。给了 `offset`/`limit` 任一且超限 → 错误 `read_output_too_many_tokens`；
//    两个都不给且超限 → 成功 + **部分视图**。256KB 大小上限只在「两个都不给」的路径上生效。
// 4. **部分视图的行数规则**：见本文件 `partial` 组的样本与结论（`p*` 探测把规则钉住）。
/** 大文件：2500 行 × 60 字符（152499 字节 → 50833 tokens，刚好越过 25000 的预算）。 */
const BIG_LINES = Array.from({ length: 2500 }, () => "z".repeat(60)).join(String.fromCharCode(10));
/** 单行 60000 字符（无换行）：观察「超长行不按列宽截断」。 */
const ONE_LONG_LINE = "q".repeat(60_000);

import { runGroup, type ProbeGroup } from "./probe-support.mjs";

/**
 * `Read` 的边角入参：行号编号与 `offset`/`limit` 边界、截断上限、二进制判定、`\r` 处理。
 *
 * 为什么每个探测用**不同的文件名**：一组探测共用一个会话与一个工作目录，同名会互相覆盖。
 */
function readGroup(): Group {
  const three = "1\n2\n3\n";
  return {
    tool: "Read",
    name: "read",
    probes: [
      {
        // `offset: 0` 是实测怪癖：从 **0** 开始编号、且从第 1 行取起（起点 = max(offset-1, 0)）。
        label: "offset-0",
        seed: { "o0.txt": three },
        args: { file_path: "${cwd}/o0.txt", offset: 0 },
      },
      // 未知入参键：声明的 `additionalProperties: false` **没有**被执行，实测照样成功。
      {
        label: "unknown-key",
        seed: { "o9.txt": three },
        args: { file_path: "${cwd}/o9.txt", nope: 1 },
      },
      {
        label: "offset-1",
        seed: { "o1.txt": three },
        args: { file_path: "${cwd}/o1.txt", offset: 1 },
      },
      {
        // 没有结尾换行的文件：仍然按「末尾空段」规则给出 2 行。
        label: "no-trailing-newline",
        seed: { "nt.txt": "a" + String.fromCharCode(10) + "b" },
        args: { file_path: "${cwd}/nt.txt" },
      },
      // 多字节正文：`originalBytes` / `returnedBytes` 是 **UTF-8 字节数**（JS 的 `.length` 会更小）。
      {
      label: "utf8-multibyte",
      seed: { "u8.txt": "中文行" + String.fromCharCode(10) + "second" + String.fromCharCode(10) },
      args: { file_path: "${cwd}/u8.txt" },
      },
      // `limit <= 0` 走 zod 精化失败（`limit: 0` 的值在夹具 `read-error-paths` 里逐字录到）。
      {
      label: "limit-negative",
      seed: { "ln.txt": "1" + String.fromCharCode(10) + "2" + String.fromCharCode(10) },
      args: { file_path: "${cwd}/ln.txt", limit: -1 },
      },
      {
        label: "offset-3",
        seed: { "o3.txt": three },
        args: { file_path: "${cwd}/o3.txt", offset: 3 },
      },
      {
        label: "offset-4",
        seed: { "o4.txt": three },
        args: { file_path: "${cwd}/o4.txt", offset: 4 },
      },
      {
        label: "offset-5",
        seed: { "o5.txt": three },
        args: { file_path: "${cwd}/o5.txt", offset: 5 },
      },
      {
        label: "offset-1-limit-2",
        seed: { "o6.txt": three },
        args: { file_path: "${cwd}/o6.txt", offset: 1, limit: 2 },
      },
      {
        label: "offset-2-limit-99",
        seed: { "o7.txt": three },
        args: { file_path: "${cwd}/o7.txt", offset: 2, limit: 99 },
      },
      {
        label: "offset-2-limit-1",
        seed: { "o8.txt": three },
        args: { file_path: "${cwd}/o8.txt", offset: 2, limit: 1 },
      },
      // 截断：默认上限由「2500 行 × 60 字符」与「单行 60000 字符」两个方向探。
      {
        label: "big-2500-lines",
        seed: { "big.txt": BIG_LINES },
        args: { file_path: "${cwd}/big.txt" },
      },
      {
        label: "big-2500-limit-3000",
        seed: { "big2.txt": BIG_LINES },
        args: { file_path: "${cwd}/big2.txt", limit: 3000 },
      },
      {
        label: "one-long-line-60000",
        seed: { "long.txt": ONE_LONG_LINE },
        args: { file_path: "${cwd}/long.txt" },
      },
      // 二进制判定：扩展名 vs 内容，两个方向都要探。
      {
        label: "ascii-named-bin",
        seed: { "ascii.bin": "plain ascii text\n" },
        args: { file_path: "${cwd}/ascii.bin" },
      },
      {
        // 注意**不要**把这种文件叫 `nul.txt`：Windows 上 `nul.<任何扩展名>` 是老式 NUL **设备名**，
        // 写进去的字节会被丢弃、读出来是空——那会录到一个假观察（本探针第一版就踩了这个坑）。
        label: "nul-bytes-named-txt",
        seedBytes: { "nulbytes.txt": Buffer.from([0x61, 0x00, 0x62, 0x0a]) },
        args: { file_path: "${cwd}/nulbytes.txt" },
      },
      {
        label: "utf8-named-bin",
        seed: { "u8.bin": "中文行\nsecond\n" },
        args: { file_path: "${cwd}/u8.bin" },
      },
      // `\r` 处理：行尾与行内。
      {
        label: "interior-cr",
        seed: { "cr.txt": "a\rb\nc\n" },
        args: { file_path: "${cwd}/cr.txt" },
      },
      {
        label: "lone-cr-middle",
        seed: { "crm.txt": "x\n\ry\n" },
        args: { file_path: "${cwd}/crm.txt" },
      },
      // 退化输入。
      { label: "single-newline", seed: { "nl.txt": "\n" }, args: { file_path: "${cwd}/nl.txt" } },
      {
        label: "empty-offset-1-limit-1",
        seed: { "e.txt": "" },
        args: { file_path: "${cwd}/e.txt", offset: 1, limit: 1 },
      },
      {
        label: "offset-string",
        seed: { "t.txt": three },
        args: { file_path: "${cwd}/t.txt", offset: "2" },
      },
      { label: "empty-file-path", seed: { "t2.txt": three }, args: { file_path: "" } },
    ],
  };
}

/**
 * **截断（token 预算）**：`Read` 的超大文件路径不是按行数截断，而是按 **token 估算**。
 *
 * 已知事实（2026-10-16）：2500 行 × 60 字符（文件 152499 字节）报
 * 「50833 estimated tokens, limit 25000」，而 `152499 / 3 = 50833` **整除**——
 * 所以估算口径很可能是 `文件字节数 / 3`。本组用几组精心挑的尺寸把这条口径钉死：
 *
 * | 探测 | 文件字节 | 字节/3 | 期望 |
 * | --- | --- | --- | --- |
 * | `bytes-74999` | 74999 | 24999 | 未超限 → 全文 |
 * | `bytes-75000` | 75000 | 25000 | 判 `>` 还是 `>=` |
 * | `bytes-75001` | 75001 | 25000（下取整）/ 25001（上取整） | 判估算是否上取整 |
 * | `bytes-75003` | 75003 | 25001 | 超限 → 部分视图 |
 * | `wide-2500x120` | 302499 | 100833 | 第二个「部分视图行数」数据点 |
 * | `slim-2500x60` | 152499 | 50833 | 取**完整**截断提示文案（head 打印 500 字符） |
 */
function truncGroup(): Group {
  return {
    tool: "Read",
    name: "trunc",
    probes: [
      {
        label: "slim-2500x60",
        seed: { "slim.txt": Array.from({ length: 2500 }, () => "z".repeat(60)).join("\n") },
        args: { file_path: "${cwd}/slim.txt" },
      },
      {
        label: "bytes-74999",
        seed: { "b74999.txt": Array.from({ length: 2500 }, () => "z".repeat(29)).join("\n") },
        args: { file_path: "${cwd}/b74999.txt" },
      },
      {
        label: "bytes-75000",
        seed: { "b75000.txt": "z".repeat(75_000) },
        args: { file_path: "${cwd}/b75000.txt" },
      },
      {
        label: "bytes-75001",
        seed: { "b75001.txt": "z".repeat(75_001) },
        args: { file_path: "${cwd}/b75001.txt" },
      },
      {
        label: "bytes-75003",
        seed: { "b75003.txt": "z".repeat(75_003) },
        args: { file_path: "${cwd}/b75003.txt" },
      },
      {
        label: "wide-2500x120",
        seed: { "wide.txt": Array.from({ length: 2500 }, () => "z".repeat(120)).join("\n") },
        args: { file_path: "${cwd}/wide.txt" },
      },
      {
        label: "slim-limit-1000",
        seed: { "slim2.txt": Array.from({ length: 2500 }, () => "z".repeat(60)).join("\n") },
        args: { file_path: "${cwd}/slim2.txt", limit: 1000 },
      },
      {
        label: "slim-offset-1000-limit-500",
        seed: { "slim3.txt": Array.from({ length: 2500 }, () => "z".repeat(60)).join("\n") },
        args: { file_path: "${cwd}/slim3.txt", offset: 1000, limit: 500 },
      },
    ],
  };
}

/**
 * 收尾问题（只问「实现时非它不可」的几条），一次性跑完：
 *
 * 1. **256KB 文件大小上限**与显式 `limit` 的先后：300KB 文件 + `limit: 5`。
 * 2. **显式 `limit` 与隐式默认的差别**：152499 字节（50833 tok）文件 + `offset:1, limit:2000`
 *    ——隐式默认同样选 2000 行却给「部分视图」，显式给的是「错误」，所以要问清这条边界。
 * 3. **入参校验的错误形状**：`offset: -1`、`limit: 1.5`（`limit: 0` 已知是 ZodError）。
 * 4. **二进制判定是否看扩展名大小写**：`.PNG`。
 * 5. **只有 `\r` 没有 `\n` 的文件**如何分行。
 */
function limitEdgeGroup(): Group {
  const slim = Array.from({ length: 2500 }, () => "z".repeat(60)).join("\n");
  const huge = Array.from({ length: 5000 }, () => "z".repeat(60)).join("\n");
  return {
    tool: "Read",
    name: "limit-edge",
    probes: [
      {
        label: "300KB-with-limit-5",
        seed: { "huge.txt": huge },
        args: { file_path: "${cwd}/huge.txt", limit: 5 },
      },
      {
        label: "50833tok-explicit-limit-2000",
        seed: { "s2000.txt": slim },
        args: { file_path: "${cwd}/s2000.txt", offset: 1, limit: 2000 },
      },
      {
        label: "offset-minus-1",
        seed: { "v1.txt": "1\n2\n3\n" },
        args: { file_path: "${cwd}/v1.txt", offset: -1 },
      },
      {
        label: "limit-fraction",
        seed: { "v2.txt": "1\n2\n3\n" },
        args: { file_path: "${cwd}/v2.txt", limit: 1.5 },
      },
      {
        label: "uppercase-png",
        seed: { "P.PNG": "not really a png\n" },
        args: { file_path: "${cwd}/P.PNG" },
      },
      { label: "only-cr", seed: { "cr.txt": "a\rb\rc" }, args: { file_path: "${cwd}/cr.txt" } },
      // 判「显式」的判据是 `limit` 还是 `offset|limit`，以及两个上限（256KB / 25000 tokens）
      // 是作用在**文件**还是**被选中的范围**上——这三条把它们钉死。
      {
        label: "50733tok-offset-only-2",
        seed: { "s-offset.txt": slim },
        args: { file_path: "${cwd}/s-offset.txt", offset: 2 },
      },
      {
        label: "304999B-implicit-vs-limit-5000",
        seed: { "h-full.txt": huge },
        args: { file_path: "${cwd}/h-full.txt", limit: 5000 },
      },
      {
        label: "302499B-limit-2000",
        seed: { "w-2000.txt": Array.from({ length: 2500 }, () => "z".repeat(120)).join("\n") },
        args: { file_path: "${cwd}/w-2000.txt", limit: 2000 },
      },
    ],
  };
}

/**
 * **隐式路径的「部分视图」**（`offset`/`limit` 都不给、文件又超 token 预算时的那一格）。
 *
 * 已知（`trunc` #1）：2500 行 × 60 字符（152499 B / 50833 tok）给的是
 * `Showing a partial view of lines 1-1045 of 2500`。本组取几组尺寸把「截到哪一行」的规则
 * 逼出来：如果被截的行数与「行数」「每行宽度」「字节数」的关系能从这些样本里解出来，
 * 那就去实现它；解不出来就如实记成窄缺口。
 *
 * 另外 `two-lines-100-75000` 是一条边界形状：总字节刚过预算、但**第二行**才是超预算的那一行。
 */
function partialViewGroup(): Group {
  const sized = (lines: number, width: number, char = "z") =>
    Array.from({ length: lines }, () => char.repeat(width)).join("\n");
  return {
    tool: "Read",
    name: "partial",
    probes: [
      {
        label: "p1-2500x60",
        seed: { "p1.txt": sized(2500, 60) },
        args: { file_path: "${cwd}/p1.txt" },
      },
      {
        label: "p2-1200x100",
        seed: { "p2.txt": sized(1200, 100) },
        args: { file_path: "${cwd}/p2.txt" },
      },
      {
        label: "p3-900x40",
        seed: { "p3.txt": sized(900, 40) },
        args: { file_path: "${cwd}/p3.txt" },
      },
      {
        label: "p4-400x250",
        seed: { "p4.txt": sized(400, 250) },
        args: { file_path: "${cwd}/p4.txt" },
      },
      {
        label: "p5-1400x200",
        seed: { "p5.txt": sized(1400, 200) },
        args: { file_path: "${cwd}/p5.txt" },
      },
      {
        label: "p6-two-lines-100-75000",
        seed: { "p6.txt": `${"a".repeat(100)}\n${"b".repeat(75000)}` },
        args: { file_path: "${cwd}/p6.txt" },
      },
      {
        label: "p7-3000x30",
        seed: { "p7.txt": sized(3000, 30) },
        args: { file_path: "${cwd}/p7.txt" },
      },
      // 二分用：宽度挑成让「每行字节数」整除一个候选预算，从而把预算窗口压到 1 字节。
      // 观察到的四组样本已经说明是「按行累加、累到某个**字节**预算」（p6 的两行文件只显示 1 行，
      // 排除了「预算 / 每行字节」的除法模型）：1045×61=63745、631×101=63731、253×251=63503、
      // 2056×31=63736 全部 ≤ 预算，而各自再多一行就超了 → 预算 ∈ [63745, 63754)。
      // 750×85 = 63750 正好落在窗口里：若 900×84 的样本显示 **750** 行，则预算 ≥ 63750；
      // 显示 749 行则预算 < 63750。1250×51 = 63750 是同一个数的第二个证据。
      // `offset: 0` 在**超预算**文件上：若给错误 `read_output_too_many_tokens`，说明「显式」的
      // 判据是「`offset` 或 `limit` 任一出现」——`offset: 0` 也算显式（把 `read.rs` 里
      // `args.get("offset").is_some()` 那个判据钉住）。
      {
        label: "s0-offset-0-over-budget",
        seed: { "s0.txt": sized(2500, 60) },
        args: { file_path: "${cwd}/s0.txt", offset: 0 },
      },
      {
        label: "s1-900x84",
        seed: { "s1.txt": sized(900, 84) },
        args: { file_path: "${cwd}/s1.txt" },
      },
      {
        label: "s2-1400x50",
        seed: { "s2.txt": sized(1400, 50) },
        args: { file_path: "${cwd}/s2.txt" },
      },
    ],
  };
}

/**
 * **扩展名怎么分流**（读前预检 / 图片 / 文本）。用来把 `read.rs` 的三张表从「只观察到一个值」
 * 扩成**观察过的集合**，并把没观察到的扩展名交给 fail-loud 标记而不是走文本路径。
 */
function extensionGroup(): Group {
  const text = "hello" + String.fromCharCode(10);
  const textLike = [
    "e.rs",
    "e.json",
    "e.ts",
    "e.md",
    "e.py",
    "e.html",
    "e.css",
    "e.yaml",
    "Makefile",
    ".gitignore",
    "e.log",
    "e.cs",
    "e.txt",
  ];
  const other = ["e.png", "e.jpg", "e.gif", "e.webp", "e.bmp", "e.pdf", "e.zip", "e.exe", "e.bin"];
  const probes: Probe[] = [
    ...textLike.map((name) => ({
      label: "text-" + name,
      seed: { [name]: text },
      args: { file_path: "${cwd}/" + name },
    })),
    ...other.map((name) => ({
      label: "other-" + name,
      seed: { [name]: text },
      args: { file_path: "${cwd}/" + name },
    })),
  ];
  return { tool: "Read", name: "ext", probes };
}

const GROUPS: Record<string, () => ProbeGroup> = {
  read: readGroup,
  trunc: truncGroup,
  "limit-edge": limitEdgeGroup,
  partial: partialViewGroup,
  ext: extensionGroup,
};

const requested = process.argv.slice(2);
const names = requested.length > 0 ? requested : Object.keys(GROUPS);
for (const name of names) {
  const build = GROUPS[name];
  if (!build) {
    process.stdout.write(`未知组名：${name}
`);
    continue;
  }
  const probes = await runGroup(build());
  process.stdout.write(`
##### ${name}: ${probes} 个探针 #####
`);
}
