// 安全改名：11 对「导出符号集一致」的 zcode*/mode* 文件——
// 改写调用点说明符 → 删除 zcode 文件。
import { readFileSync, writeFileSync, rmSync, existsSync } from "node:fs";
import { execFileSync } from "node:child_process";

const PAIRS = [
  "packages/desktop/src/main/zcodeDataSizeWorker.ts",
  "packages/server/src/remote/zcodeAgentBundleWrapper.ts",
  "packages/server/src/remote/zcodeAgentOfficialPluginAssets.ts",
  "packages/server/src/remote/zcodeAgentOfficialPluginPermissionRepair.ts",
  "packages/server/src/remote/zcodeAgentWrapperDeploy.ts",
  "packages/ui/src/lib/zcodeCustomModelValue.ts",
  "packages/ui/src/lib/zcodeDraftSkillInvalidation.ts",
  "packages/ui/src/lib/zcodeTaskMetaMerge.ts",
  "packages/ui/src/store/zcodeSessionStoreNavigation.ts",
  "packages/ui/src/store/zcodeSessionStoreTaskSlice.ts",
  "packages/ui/src/store/zcodeSessionStoreWorkspaceSlice.ts",
];

const all = execFileSync("git", ["ls-files"], { maxBuffer: 1 << 28 })
  .toString()
  .split("\n")
  .filter((f) => /\.(ts|tsx)$/.test(f) && !f.includes("/dist/") && !f.includes("/out/") && !f.includes("/node_modules/"));

let touched = 0;
for (const z of PAIRS) {
  const zbase = z.split("/").pop().replace(/\.ts$/, "");
  const mbase = zbase.replace(/^zcode/, "mode");
  for (const f of all) {
    if (f === z) continue;
    let text;
    try {
      text = readFileSync(f, "utf8");
    } catch {
      continue;
    }
    if (!text.includes(`/${zbase}.js`)) continue;
    const next = text.split(`/${zbase}.js`).join(`/${mbase}.js`);
    if (next === text) continue;
    writeFileSync(f, next);
    touched++;
    console.log(`  改写 ${f}: ${zbase}.js → ${mbase}.js`);
  }
  if (existsSync(z)) {
    rmSync(z);
    console.log(`  删除 ${z}`);
  }
}
console.log(`\n共改写 ${touched} 个文件的说明符，删除 ${PAIRS.length} 个 zcode 文件`);
