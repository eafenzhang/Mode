// Build the MV3 extension: TypeScript -> dist/*.js, then copy manifest/popup.html.
// `dist/` is gitignored; only TS/HTML/JSON sources are tracked (language policy).
import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync, rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const packageRoot = dirname(dirname(fileURLToPath(import.meta.url)));
const repoRoot = join(packageRoot, "..", "..");
const dist = join(packageRoot, "dist");
const tsc = join(repoRoot, "node_modules", "typescript", "bin", "tsc");

rmSync(dist, { recursive: true, force: true });
mkdirSync(dist, { recursive: true });
execFileSync(process.execPath, [tsc, "-p", join(packageRoot, "tsconfig.json")], {
  stdio: "inherit",
});
for (const file of ["manifest.json", "popup.html"]) {
  copyFileSync(join(packageRoot, "src", file), join(dist, file));
}
console.log(`[browser-import-extension] built ${dist}`);
