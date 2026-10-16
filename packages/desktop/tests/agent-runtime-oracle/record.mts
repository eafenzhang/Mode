// M2 Task 0：行为夹具的录制入口（spec §3.1）。
//
// 用法：pnpm exec tsx packages/desktop/tests/agent-runtime-oracle/record.mts
//
// 只有这里是「写夹具」的地方：先用旧运行时跑一遍、把观测到的三层投影写进 `expect`。
// 手写 `expect` 等于把「我以为的行为」当规格，正是 clean-room 要避免的。
import { FIXTURE_CASES, observeCase, writeFixture } from "./fixtures.mjs";

for (const fixtureCase of FIXTURE_CASES) {
  process.stdout.write(`录制 ${fixtureCase.case} …\n`);
  const observed = await observeCase(fixtureCase);
  await writeFixture({ ...fixtureCase, expect: observed });
  process.stdout.write(
    `  ${fixtureCase.case}: ${observed.toolCalls.length} 个工具调用、` +
      `${Object.keys(observed.files).length} 个文件、${observed.events.length} 条会话事件\n`,
  );
}
