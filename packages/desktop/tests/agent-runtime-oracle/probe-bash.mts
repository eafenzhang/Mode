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
      label: "timeout-1.5s",
      args: { command: "sleep 3", timeout: 1500, description: "Sleep longer than 1.5s timeout" },
    },
    {
      label: "output-then-exit-7",
      args: { command: "printf 'before\\n'; exit 7", description: "Write output then exit 7" },
    },
    {
      label: "timeout-1s",
      args: { command: "sleep 5", timeout: 1000, description: "Sleep longer than timeout" },
    },
    {
      label: "missing-command",
      args: {},
    },
    {
      label: "mixed-streams",
      args: {
        command: "printf 'out1\\n'; printf 'err1\\n' >&2; printf 'out2\\n'",
        description: "Interleave stdout and stderr",
      },
    },
    {
      label: "trailing-newlines",
      args: {
        command: "printf 'a\\n\\n\\n'",
        description: "Write trailing newlines",
      },
    },
    {
      label: "trailing-spaces",
      args: {
        command: "printf 'a   \\n'",
        description: "Write trailing spaces",
      },
    },
    {
      label: "empty-output",
      args: { command: "true", description: "Exit successfully with no output" },
    },
    {
      label: "is-tty",
      args: {
        command:
          "if [ -t 1 ]; then printf 'stdout:tty'; else printf 'stdout:pipe'; fi; " +
          "if [ -t 0 ]; then printf ' stdin:tty'; else printf ' stdin:pipe'; fi",
        description: "Report whether stdout and stdin are TTYs",
      },
    },
    {
      label: "timeout-child",
      args: {
        command: "sleep 30 & echo $! > child.pid; wait",
        timeout: 1000,
        description: "Start a child then wait past the timeout",
      },
    },
    {
      label: "child-alive-after-timeout",
      args: {
        command:
          "if kill -0 $(cat child.pid) 2>/dev/null; then printf alive; else printf dead; fi",
        description: "Check whether the timed-out child still exists",
      },
    },
    {
      label: "child-alive-after-timeout-2s",
      args: {
        command:
          "sleep 2; if kill -0 $(cat child.pid) 2>/dev/null; then printf alive; else printf dead; fi",
        description: "Check the timed-out child again after two seconds",
      },
    },
    {
      label: "large-output-4k",
      args: {
        command: "head -c 4096 /dev/zero | tr '\\0' 'e'",
        description: "Write 4096 bytes to stdout",
      },
    },
    {
      label: "large-output-8k",
      args: {
        command: "head -c 8192 /dev/zero | tr '\\0' 'f'",
        description: "Write 8192 bytes to stdout",
      },
    },
    {
      label: "large-output-16k",
      args: {
        command: "head -c 16384 /dev/zero | tr '\\0' 'g'",
        description: "Write 16384 bytes to stdout",
      },
    },
    {
      label: "large-output-20k",
      args: {
        command: "head -c 20480 /dev/zero | tr '\\0' 'i'",
        description: "Write 20480 bytes to stdout",
      },
    },
    {
      label: "large-output-24k",
      args: {
        command: "head -c 24576 /dev/zero | tr '\\0' 'j'",
        description: "Write 24576 bytes to stdout",
      },
    },
    {
      label: "large-output-28k",
      args: {
        command: "head -c 28672 /dev/zero | tr '\\0' 'k'",
        description: "Write 28672 bytes to stdout",
      },
    },
    {
      label: "large-output-30000",
      args: {
        command: "head -c 30000 /dev/zero | tr '\\0' 'l'",
        description: "Write 30000 bytes to stdout",
      },
    },
    {
      label: "large-output-30720",
      args: {
        command: "head -c 30720 /dev/zero | tr '\\0' 'm'",
        description: "Write 30720 bytes to stdout",
      },
    },
    {
      label: "large-output-32k",
      args: {
        command: "head -c 32768 /dev/zero | tr '\\0' 'h'",
        description: "Write 32768 bytes to stdout",
      },
    },
    {
      label: "large-output-64k",
      args: {
        command: "head -c 65536 /dev/zero | tr '\\0' 'a'",
        description: "Write 65536 bytes to stdout",
      },
    },
    {
      label: "large-output-100k",
      args: {
        command: "head -c 100000 /dev/zero | tr '\\0' 'b'",
        description: "Write 100000 bytes to stdout",
      },
    },
    {
      label: "large-output-128k",
      args: {
        command: "head -c 131072 /dev/zero | tr '\\0' 'c'",
        description: "Write 131072 bytes to stdout",
      },
    },
    {
      label: "large-output-160k",
      args: {
        command: "head -c 160000 /dev/zero | tr '\\0' 'd'",
        description: "Write 160000 bytes to stdout",
      },
    },
    {
      label: "large-output-200k",
      args: {
        command: "head -c 200000 /dev/zero | tr '\\0' 'z'",
        description: "Write 200000 bytes to stdout",
      },
    },
    {
      label: "large-output-260k",
      args: {
        command: "head -c 260000 /dev/zero | tr '\\0' 'w'",
        description: "Write 260000 bytes to stdout",
      },
    },
    {
      label: "large-output-300k",
      args: {
        command: "head -c 300000 /dev/zero | tr '\\0' 'x'",
        description: "Write 300000 bytes to stdout",
      },
    },
    {
      label: "large-output-1m",
      args: {
        command: "head -c 1000000 /dev/zero | tr '\\0' 'y'",
        description: "Write 1000000 bytes to stdout",
      },
    },
  ],
});

process.stdout.write(`\n（共 ${count} 组）\n`);
