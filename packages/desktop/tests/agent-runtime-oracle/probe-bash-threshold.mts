// M2 Task 4 Step 0 收尾：`Bash` 大输出阈值的**精确字节**。
//
// 用法：
//   pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/probe-bash-threshold.mts
//
// 已知：30,000 直出、30,720 持久化（spec §3.4）。本探针在边界附近逐字节取样。
import { runGroup } from "./probe-support.mjs";

const sizes = [29999, 30000, 30001, 30718, 30719, 30720, 30721, 30722, 31000];

await runGroup({
  tool: "Bash",
  name: "bash-threshold",
  probes: sizes.map((size) => ({
    label: `bytes-${size}`,
    args: {
      command: `head -c ${size} /dev/zero | tr '\\0' 'x'`,
      description: `Emit ${size} bytes`,
    },
  })),
});
