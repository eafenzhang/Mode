/**
 * M4 T1 台账生成器的纯函数用例（计划：`docs/plans/2026-10-18-m4-third-party-ledger.md`）。
 *
 * 生成器会读 cargo registry 缓存并写候选文件；这里只钉不依赖磁盘的口径：
 * Cargo.lock 解析、许可证文件识别、内容哈希命名。
 */
import assert from "node:assert/strict";
import test from "node:test";

import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  classifyLicenseText,
  indexStandardLicenseTexts,
  licenseMemberNames,
  parseLockPackages,
  parseSpdxIdentifiers,
  requiresHumanReview,
  selectableLicenseIdentifiers,
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

test("SPDX 表达式拆分覆盖 OR 与斜杠写法", () => {
  assert.deepEqual(parseSpdxIdentifiers("MIT OR Apache-2.0"), ["MIT", "Apache-2.0"]);
  assert.deepEqual(parseSpdxIdentifiers("MIT/Apache-2.0"), ["MIT", "Apache-2.0"]);
  assert.deepEqual(parseSpdxIdentifiers("Zlib OR Apache-2.0 OR MIT"), [
    "Zlib",
    "Apache-2.0",
    "MIT",
  ]);
  assert.deepEqual(parseSpdxIdentifiers("(MIT OR Apache-2.0)"), ["MIT", "Apache-2.0"]);
});

test("LGPL 选项不自动附文本；MPL 附文本但仍需人工 review", () => {
  assert.deepEqual(selectableLicenseIdentifiers("MPL-2.0"), ["MPL-2.0"]);
  assert.deepEqual(
    selectableLicenseIdentifiers("MIT OR Apache-2.0 OR LGPL-2.1-or-later"),
    ["MIT", "Apache-2.0"],
  );
  assert.equal(requiresHumanReview("MIT OR Apache-2.0"), false);
  assert.equal(requiresHumanReview("MPL-2.0"), true);
  assert.equal(requiresHumanReview("MIT OR Apache-2.0 OR LGPL-2.1-or-later"), true);
});

test("许可证文本按正文特征分类", () => {
  assert.equal(classifyLicenseText("Permission is hereby granted, free of charge, to any person"), "MIT");
  assert.equal(classifyLicenseText("Apache License\nVersion 2.0, January 2004"), "Apache-2.0");
  assert.equal(
    classifyLicenseText(
      "Redistribution and use in source and binary forms, with or without modification, are permitted provided that",
    ),
    "BSD-2-Clause",
  );
  assert.equal(
    classifyLicenseText(
      "Redistribution and use in source and binary forms, with or without modification, are permitted provided that the following conditions are met: Neither the name of the copyright holder",
    ),
    "BSD-3-Clause",
  );
  assert.equal(
    classifyLicenseText("This software is provided 'as-is', without any express or implied warranty."),
    "Zlib",
  );
  assert.equal(classifyLicenseText("Mozilla Public License Version 2.0"), "MPL-2.0");
  assert.equal(classifyLicenseText("unknown text"), null);
});

test("仓库标准文本索引覆盖本轮需要的四个标识", () => {
  const here = dirname(fileURLToPath(import.meta.url));
  const index = indexStandardLicenseTexts(resolve(here, "../../../scripts/license-texts"));
  for (const identifier of ["MIT", "Apache-2.0", "BSD-3-Clause", "Zlib"]) {
    assert.ok(index.has(identifier), `缺少 ${identifier} 标准文本`);
  }
});
