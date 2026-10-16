// 用本机 gnu 路线装配好环境后调用 cargo，省掉人肉 export PATH / CARGO_TARGET_*_LINKER。
//
// 用法（参数原样透传给 cargo）：
//   node scripts/rust-cargo.ts test --workspace
//   node scripts/rust-cargo.ts build --release -p mode-tauri
//   node scripts/rust-cargo.ts clippy --workspace --all-targets -- -D warnings
//
// 刻意**不注入 `--target`**：默认按宿主 toolchain 构建，产物落 `target/<profile>`
// （`mode-tauri` 因此在 `target/release/ModeTauri.exe`）。需要交叉编译时自己传
// `--target`，并用 MODE_RUST_TARGET 告知本脚本目标三元组（影响 linker 变量的选择）。
//
// 装配口径见 `scripts/rust-gnu-env.ts`；CI（msvc / Linux）上它退化成「什么都不做」，
// 所以 CI 直接用 cargo 即可，不必经本脚本。
import { spawnSync } from "node:child_process";

import { cargoHostToolchainArgs, defaultGnuTarget, withCargoEnv } from "./rust-gnu-env.ts";

const args = process.argv.slice(2);
if (args.length === 0) {
  console.error("用法：node scripts/rust-cargo.ts <cargo 子命令与参数>");
  console.error("例：node scripts/rust-cargo.ts test --workspace");
  process.exit(1);
}

// 非 Windows 平台没有 gnu 路线可装配，target 留空即让模块退化成 no-op。
const target =
  process.env.MODE_RUST_TARGET?.trim() ||
  (process.platform === "win32" ? defaultGnuTarget() : undefined);

const env = withCargoEnv(process.env, { target });
const cargoArgs = [...cargoHostToolchainArgs(env, target), ...args];

const result = spawnSync("cargo", cargoArgs, {
  cwd: process.cwd(),
  env,
  stdio: "inherit",
  // Windows 上 cargo 是 .cmd/.exe 入口，交给 shell 解析更稳（同 mise-run.mjs 的口径）。
  shell: process.platform === "win32",
});

if (result.error) {
  console.error(`[rust-cargo] 无法执行 cargo：${result.error.message}（确认 cargo 已在 PATH）`);
  process.exit(1);
}
if (result.signal) {
  process.kill(process.pid, result.signal);
}
process.exit(result.status ?? 1);
