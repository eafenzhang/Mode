// M2 Task 3：**写工具**（`Write` / `Edit`）的行为夹具。
//
// 为什么单独一个文件：oxlint 的 `max-lines` 上限是 400 行（**错误级**，会卡住 release 的
// verify），Task 3 追加 17 条夹具后 `fixtures.mts` 到了 495 行——与 Task 2 把探针拆成
// `probe-read*` 几个文件是同一条理由。
//
// 纪律不变：`prompt` / `seed` / `script` 是**输入**（人写）；`expect` 一律由
// `record.mts` 从 oracle 录，人不得手改。
import type { FixtureCase } from "./fixtures.mjs";

export const WRITE_EDIT_FIXTURE_CASES: readonly FixtureCase[] = [
  // ---- Task 3 追加：`Write` 的行为（spec §3.3 的观察出处）。
  //      剧本与种子是**输入**（人写）；`expect` 一律由 `record.mts` 从 oracle 录。
  {
    case: "write-new",
    prompt: "用 Write 工具把 made.txt 创建出来，然后回一句话说明结果。",
    seed: {},
    script: [
      { toolCalls: [{ name: "Write", input: { file_path: "${cwd}/made.txt", content: "hello write\nsecond line\n" } }] },
      { text: "made.txt 创建成功。" },
    ],
  },
  {
    case: "write-overwrite-unread",
    prompt: "用 Write 覆盖 existing.txt，然后说明结果。",
    seed: { "existing.txt": "original\n" },
    // 关键：**不先 Read**。观察到的是失败（`File has not been read yet.…`）。
    script: [
      { toolCalls: [{ name: "Write", input: { file_path: "${cwd}/existing.txt", content: "replaced\n" } }] },
      { text: "覆盖失败了。" },
    ],
  },
  {
    case: "write-overwrite-read",
    prompt: "先 Read 再 Write 覆盖 existing.txt，然后说明结果。",
    seed: { "existing.txt": "original\n" },
    script: [
      { toolCalls: [{ name: "Read", input: { file_path: "${cwd}/existing.txt" } }] },
      { toolCalls: [{ name: "Write", input: { file_path: "${cwd}/existing.txt", content: "replaced\n" } }] },
      { text: "先读后写，成功了。" },
    ],
  },
  {
    case: "write-empty-args",
    prompt: "用一个空的入参调用 Write，然后说明结果。",
    seed: {},
    // 正对照：空入参拿到的是 `inputSchema validation`（不是 `tool_not_found`），
    // 即 `Write` **确实注册了**（同 Task 2 用 `Agent` 做正对照的思路）。
    script: [{ toolCalls: [{ name: "Write", input: {} }] }, { text: "入参不合法。" }],
  },
  {
    // 相对路径的入参：**成功文案里的路径是原样回显还是解析后的绝对路径**——
    // 这条决定了实现要不要先解析再拼消息，不能靠猜（`Read` 的实测是「按 cwd 解析」，
    // 但那是**读**的路径，写成功文案的拼法未必同源）。实测：成功文案里是**原样回显**。
    case: "write-relative-path",
    prompt: "用相对路径调用 Write 创建 rel-out.txt，然后说明结果。",
    seed: {},
    script: [
      { toolCalls: [{ name: "Write", input: { file_path: "rel-out.txt", content: "rel\n" } }] },
      { text: "写完了。" },
    ],
  },
  {
    // 只给 `file_path`、不给 `content`：把「缺必填字段」与「字段不是字符串」两条校验分支钉住，
    // 免得实现自己发明一个错误形状。
    case: "write-missing-content",
    prompt: "只给 file_path 调用 Write，然后说明结果。",
    seed: {},
    script: [
      { toolCalls: [{ name: "Write", input: { file_path: "${cwd}/only-path.txt" } }] },
      { text: "缺 content。" },
    ],
  },
  {
    // 空字符串路径：**不是**「缺字段」，是否走校验分支要看实测（`Read` 那条是走校验错误的）。
    case: "write-empty-path",
    prompt: "用空字符串路径调用 Write，然后说明结果。",
    seed: {},
    script: [
      { toolCalls: [{ name: "Write", input: { file_path: "", content: "x\n" } }] },
      { text: "空路径。" },
    ],
  },
  // ---- Task 3 Task B：`Edit` 的行为（spec §3.3.1 的观察出处）。
  //      覆盖六个错误码（1/3/4/6/8/9）+ 三种成功文案 + 一条入参校验。
  {
    case: "edit-unique",
    prompt: "先 Read 再 Edit 把 Beta 换成 BETA，然后说明结果。",
    seed: { "target.txt": "alpha\nBeta\ngamma\n" },
    script: [
      { toolCalls: [{ name: "Read", input: { file_path: "${cwd}/target.txt" } }] },
      {
        toolCalls: [
          {
            name: "Edit",
            input: { file_path: "${cwd}/target.txt", old_string: "Beta", new_string: "BETA" },
          },
        ],
      },
      { text: "改好了。" },
    ],
  },
  {
    case: "edit-replace-all",
    prompt: "先 Read 再 Edit，用 replace_all 把所有 foo 换成 bar。",
    seed: { "target.txt": "x\nfoo\ny\nfoo\n" },
    script: [
      { toolCalls: [{ name: "Read", input: { file_path: "${cwd}/target.txt" } }] },
      {
        toolCalls: [
          {
            name: "Edit",
            input: {
              file_path: "${cwd}/target.txt",
              old_string: "foo",
              new_string: "bar",
              replace_all: true,
            },
          },
        ],
      },
      { text: "全换了。" },
    ],
  },
  {
    // 判据 5（`code=8`）。
    case: "edit-not-found",
    prompt: "先 Read 再 Edit 一个不存在的串，然后说明结果。",
    seed: { "target.txt": "alpha\nBeta\n" },
    script: [
      { toolCalls: [{ name: "Read", input: { file_path: "${cwd}/target.txt" } }] },
      {
        toolCalls: [
          {
            name: "Edit",
            input: { file_path: "${cwd}/target.txt", old_string: "Delta", new_string: "D" },
          },
        ],
      },
      { text: "没找到。" },
    ],
  },
  {
    // 判据 6（`code=9`）：纯子串计数——`"  indented"` 与 `"not indented"` 里的 `indented` 都算命中。
    case: "edit-ambiguous",
    prompt: "先 Read 再 Edit 一个出现两次的串，然后说明结果。",
    seed: { "target.txt": "  indented\nnot indented\n" },
    script: [
      { toolCalls: [{ name: "Read", input: { file_path: "${cwd}/target.txt" } }] },
      {
        toolCalls: [
          {
            name: "Edit",
            input: { file_path: "${cwd}/target.txt", old_string: "indented", new_string: "X" },
          },
        ],
      },
      { text: "有歧义。" },
    ],
  },
  {
    // 判据 1（`code=1`），且**与文件状态无关**（这里的文件是存在的）。
    case: "edit-same-string",
    prompt: "先 Read 再 Edit，把 alpha 换成 alpha，然后说明结果。",
    seed: { "target.txt": "alpha\n" },
    script: [
      { toolCalls: [{ name: "Read", input: { file_path: "${cwd}/target.txt" } }] },
      {
        toolCalls: [
          {
            name: "Edit",
            input: { file_path: "${cwd}/target.txt", old_string: "alpha", new_string: "alpha" },
          },
        ],
      },
      { text: "没改动。" },
    ],
  },
  {
    // 判据 3（`code=6`）：不先 Read。
    case: "edit-no-read",
    prompt: "直接 Edit（不先 Read），然后说明结果。",
    seed: { "target.txt": "gamma\n" },
    script: [
      {
        toolCalls: [
          {
            name: "Edit",
            input: { file_path: "${cwd}/target.txt", old_string: "gamma", new_string: "G" },
          },
        ],
      },
      { text: "没读过。" },
    ],
  },
  {
    // 判据 2（`code=4`）：文件不存在且 `old_string` 非空（且没先 Read）。
    case: "edit-missing-file",
    prompt: "Edit 一个不存在的文件，然后说明结果。",
    seed: {},
    script: [
      {
        toolCalls: [
          {
            name: "Edit",
            input: { file_path: "${cwd}/never-existed.txt", old_string: "x", new_string: "y" },
          },
        ],
      },
      { text: "文件不存在。" },
    ],
  },
  {
    // 判据 2'：空 `old_string` + 文件不存在 = **新建**（成功）。
    case: "edit-empty-old-creates",
    prompt: "用空 old_string 的 Edit 建一个新文件，然后说明结果。",
    seed: {},
    script: [
      {
        toolCalls: [
          {
            name: "Edit",
            input: { file_path: "${cwd}/created.txt", old_string: "", new_string: "created\n" },
          },
        ],
      },
      { text: "建好了。" },
    ],
  },
  {
    // 判据 4（`code=3`）：空 `old_string` + 文件已存在（先读过）。
    case: "edit-empty-old-existing",
    prompt: "先 Read 再用空 old_string 的 Edit，然后说明结果。",
    seed: { "target.txt": "zeta\n" },
    script: [
      { toolCalls: [{ name: "Read", input: { file_path: "${cwd}/target.txt" } }] },
      {
        toolCalls: [
          {
            name: "Edit",
            input: { file_path: "${cwd}/target.txt", old_string: "", new_string: "Z" },
          },
        ],
      },
      { text: "文件已存在。" },
    ],
  },
  {
    // 入参校验：缺 `new_string`。
    case: "edit-missing-new-string",
    prompt: "只给 file_path 与 old_string 调用 Edit，然后说明结果。",
    seed: { "target.txt": "alpha\n" },
    script: [
      {
        toolCalls: [
          { name: "Edit", input: { file_path: "${cwd}/target.txt", old_string: "alpha" } },
        ],
      },
      { text: "缺 new_string。" },
    ],
  },
];
