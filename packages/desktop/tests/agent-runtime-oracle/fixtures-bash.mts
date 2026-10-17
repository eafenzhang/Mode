// M2 Task 4：`Bash` 前台行为夹具。
//
// 纪律不变：`prompt` / `seed` / `script` 是**输入**（人写）；`expect` 一律由
// `record.mts` 从 oracle 录，人不得手改。
//
// 覆盖面来自 spec §3.4 的黑盒观察：成功输出、非零退出、stderr 合并、stdout/stderr 交错、
// 空输出、命令不存在、超时（1s 与 1.5s 的秒数格式）、入参校验、尾随空白。
// `run_in_background` 与 >30KB 的持久化包装**不进夹具**（前者未观察；后者的路径含数据根，
// 夹具无法稳定命中——那部分由 `bash.rs` 的单元测试钉）。
import type { FixtureCase } from "./fixtures.mjs";

export const BASH_FIXTURE_CASES: readonly FixtureCase[] = [
  {
    case: "bash-stdout",
    prompt: "用 Bash 跑一个 printf，然后说明结果。",
    seed: {},
    script: [
      {
        toolCalls: [
          {
            name: "Bash",
            input: { command: "printf 'hello\\nworld\\n'", description: "Print two lines" },
          },
        ],
      },
      { text: "输出完成。" },
    ],
  },
  {
    case: "bash-exit-7",
    prompt: "用 Bash 执行 exit 7，然后说明结果。",
    seed: {},
    script: [
      {
        toolCalls: [{ name: "Bash", input: { command: "exit 7", description: "Exit with code 7" } }],
      },
      { text: "退出码是 7。" },
    ],
  },
  {
    case: "bash-output-then-exit-7",
    prompt: "用 Bash 先打印 before 再以 7 退出，然后说明结果。",
    seed: {},
    script: [
      {
        toolCalls: [
          {
            name: "Bash",
            input: { command: "printf 'before\\n'; exit 7", description: "Print then exit 7" },
          },
        ],
      },
      { text: "先有输出再退出。" },
    ],
  },
  {
    case: "bash-stderr",
    prompt: "用 Bash 往 stderr 写一行，然后说明结果。",
    seed: {},
    script: [
      {
        toolCalls: [
          {
            name: "Bash",
            input: { command: "printf 'oops\\n' >&2", description: "Write to stderr" },
          },
        ],
      },
      { text: "stderr 合并了。" },
    ],
  },
  {
    case: "bash-mixed-streams",
    prompt: "用 Bash 交错写 stdout/stderr，然后说明结果。",
    seed: {},
    script: [
      {
        toolCalls: [
          {
            name: "Bash",
            input: {
              command: "printf 'out1\\n'; printf 'err1\\n' >&2; printf 'out2\\n'",
              description: "Interleave stdout and stderr",
            },
          },
        ],
      },
      { text: "交错输出完成。" },
    ],
  },
  {
    case: "bash-empty-output",
    prompt: "用 Bash 跑 true，然后说明结果。",
    seed: {},
    script: [
      {
        toolCalls: [{ name: "Bash", input: { command: "true", description: "Exit with no output" } }],
      },
      { text: "没有输出。" },
    ],
  },
  {
    case: "bash-trailing-whitespace",
    prompt: "用 Bash 输出带尾随空格的内容，然后说明结果。",
    seed: {},
    script: [
      {
        toolCalls: [
          {
            name: "Bash",
            input: { command: "printf 'a   \\n'", description: "Write trailing spaces" },
          },
        ],
      },
      { text: "尾随空白处理完毕。" },
    ],
  },
  {
    case: "bash-command-not-found",
    prompt: "用 Bash 调用一个不存在的命令，然后说明结果。",
    seed: {},
    script: [
      {
        toolCalls: [
          {
            name: "Bash",
            input: {
              command: "definitely_not_a_command_xyz",
              description: "Run a missing command",
            },
          },
        ],
      },
      { text: "命令不存在。" },
    ],
  },
  {
    case: "bash-timeout",
    prompt: "用 Bash 跑一个超过超时的命令，然后说明结果。",
    seed: {},
    script: [
      {
        toolCalls: [
          {
            name: "Bash",
            input: { command: "sleep 5", timeout: 1000, description: "Sleep longer than timeout" },
          },
        ],
      },
      { text: "超时了。" },
    ],
  },
  {
    case: "bash-timeout-fractional",
    prompt: "用 Bash 跑一个 1.5 秒超时的命令，然后说明结果。",
    seed: {},
    script: [
      {
        toolCalls: [
          {
            name: "Bash",
            input: { command: "sleep 3", timeout: 1500, description: "Sleep past 1.5s timeout" },
          },
        ],
      },
      { text: "1.5 秒超时了。" },
    ],
  },
  {
    case: "bash-empty-args",
    prompt: "用空入参调用 Bash，然后说明结果。",
    seed: {},
    script: [{ toolCalls: [{ name: "Bash", input: {} }] }, { text: "入参不合法。" }],
  },
];
