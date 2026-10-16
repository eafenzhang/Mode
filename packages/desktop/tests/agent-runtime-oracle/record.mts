// M2 Task 0：行为夹具的录制入口（spec §3.1）。
//
// 用法：
//   pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/record.mts            # 全部
//   pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/record.mts <case> […]  # 只录这几条
//
// 只有这里是「写夹具」的地方：先用旧运行时跑一遍、把观测到的三层投影写进 `expect`。
// 手写 `expect` 等于把「我以为的行为」当规格，正是 clean-room 要避免的。
//
// 支持只录指定用例的理由：**已验收的夹具不该被顺手重写**。某条用例的输入改了（Task 2 就遇到
// 一次：`nul.txt` 在 Windows 上是设备名，必须改名）时只重录那一条，别的逐字不动。
import { FIXTURE_CASES, observeCase, writeFixture } from "./fixtures.mjs";

const requested = process.argv.slice(2);
const cases =
  requested.length > 0
    ? FIXTURE_CASES.filter((fixtureCase) => requested.includes(fixtureCase.case))
    : FIXTURE_CASES;
const unknown = requested.filter(
  (name) => !FIXTURE_CASES.some((fixtureCase) => fixtureCase.case === name),
);
if (unknown.length > 0) {
  process.stdout.write(`未知用例：${unknown.join(", ")}\n`);
  process.exit(1);
}

for (const fixtureCase of cases) {
  process.stdout.write(`录制 ${fixtureCase.case} …\n`);
  const observed = await observeCase(fixtureCase);
  await writeFixture({ ...fixtureCase, expect: observed });
  process.stdout.write(
    `  ${fixtureCase.case}: ${observed.toolCalls.length} 个工具调用、` +
      `${Object.keys(observed.files).length} 个文件、${observed.events.length} 条会话事件\n`,
  );
}
