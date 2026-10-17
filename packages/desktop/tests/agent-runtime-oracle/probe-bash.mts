// M2 Task 4 探针：`Bash` 工具的行为面（注册、输出、退出码、超时、stderr）。
//
// 用法：
//   pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-bash.mts
//
// 纪律（docs/specs/clean-room-policy.md §黑盒对拍）：只运行旧实现、观察它的输入输出与事件，
// 不读实现体。Task 4 的计划细节（超时/取消/退出码语义）必须来自这里的观察，不能靠猜。
import { runGroup } from "./probe-support.mjs";

const count = await runGroup({
  tool: "Bash",
  name: "bash",
  probes: [
    {
      label: "stdout",
      args: { command: "printf 'hello\\nworld\\n'", description: "Print two lines" },
    },
    {
      label: "exit-code-7",
      args: { command: "exit 7", description: "Exit with code 7" },
    },
    {
      label: "stderr",
      args: { command: "printf 'oops\\n' >&2", description: "Write to stderr" },
    },
    {
      label: "cwd",
      args: { command: "pwd", description: "Print working directory" },
    },
    {
      label: "command-not-found",
      args: { command: "definitely_not_a_command_xyz", description: "Run a missing command" },
    },
    {
      label: "timeout-1s",
      args: { command: "sleep 5", timeout: 1000, description: "Sleep longer than timeout" },
    },
    {
      label: "missing-command",
      args: {},
    },
  ],
});

process.stdout.write(`\n（共 ${count} 组）\n`);
