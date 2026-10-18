/**
 * M4 T1 台账生成器的纯函数用例（计划：`docs/plans/2026-10-18-m4-third-party-ledger.md`）。
 *
 * 生成器会读 cargo registry 缓存并写候选文件；这里只钉不依赖磁盘的口径：
 * Cargo.lock 解析、许可证文件识别、内容哈希命名。
 */
import assert from "node:assert/strict";
import test from "node:test";

import {
  licenseMemberNames,
  parseLockPackages,
  sha256Hex,
} from "../../../scripts/generate-rust-ledger.ts";

test("Cargo.lock 解析 name/version/checksum，忽略 path 依赖", () => {
  const lock = `
version = 4

[[package]]
name = "alpha"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "aaaa"

[[package]]
name = "workspace-crate"
version = "0.1.0"
dependencies = ["alpha"]
`;
  assert.deepEqual(parseLockPackages(lock), [
    { name: "alpha", version: "1.0.0", checksum: "aaaa" },
    { name: "workspace-crate", version: "0.1.0", checksum: null },
  ]);
});

test("许可证文件识别覆盖常见命名，排除代码与配置", () => {
  assert.deepEqual(
    licenseMemberNames([
      "LICENSE",
      "LICENSE-MIT",
      "license-apache-2.0",
      "COPYING",
      "NOTICE",
      "UNLICENSE",
      "Cargo.toml",
      "Cargo.lock",
      "README.md",
      "license.rs",
      "license.json",
    ]),
    ["COPYING", "LICENSE", "LICENSE-MIT", "NOTICE", "UNLICENSE", "license-apache-2.0"],
  );
});

test("内容哈希是 CRLF 归一后的 sha256（与 notices 生成口径一致）", () => {
  const lf = Buffer.from("line1\nline2\n");
  const crlf = Buffer.from("line1\r\nline2\r\n");
  assert.equal(sha256Hex(lf), sha256Hex(Buffer.from(crlf.toString("utf8").replaceAll("\r\n", "\n"))));
});
