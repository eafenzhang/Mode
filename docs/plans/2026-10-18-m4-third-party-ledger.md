# M4 第三方台账：Rust 依赖覆盖缺口与处置计划（2026-10-18）

> 输入：`third-party/rust-sources.json`、根 `Cargo.lock` / `cargo metadata`、
> `scripts/licenses.mjs`、`scripts/generate-third-party-notices.mjs`、
> `.github/workflows/licenses-notices.yml`。本文件是**盘点与计划**，不包含法务结论。

## 结论先行

现有 Rust 台账**只覆盖 `crates/mode-cua-ax` 的 63 个 crate**。M2/M3 新增的运行时
（agent-core / agent-tools / plugin-host / plugin-host-adapter）与 Tauri 壳解析出的依赖图
有 **142 / 485 个 crate 不在台账里**；`THIRD-PARTY-NOTICES.md` 因此不会包含它们。
这不是「已确认违规」，而是**发布物与声明之间的覆盖缺口**，必须在 M4 台账重建时收口。

## 现状（可核对的事实）

| 项 | 事实 | 出处 |
| --- | --- | --- |
| 台账 scope | `rust-sources.json` 的 `scope` 明写「Crates compiled into the mode-cua-ax Windows cdylib」 | `third-party/rust-sources.json` |
| 台账条目 | **63** 个 crate | 同上 |
| 生成/校验输入 | `crates/mode-cua-ax/Cargo.toml` + `Cargo.lock` + `rust-sources.json` | `scripts/licenses.mjs:124-129` |
| notices 内容 | 「Rust crates」段只来自上述 63 条 | `scripts/generate-third-party-notices.mjs:124-190` |
| CI 生成路径 | Linux `licenses-notices.yml` 跑 `node scripts/licenses.mjs notices`（含 `check`） | workflow 注释：Windows/macOS 因 optional deps 缺包不能生成 |

## 缺口测量（2026-10-18，根 `Cargo.lock`）

用 `cargo metadata --format-version 1 --locked` 的 resolve 图，按根集合取可达 crate，
再与 `rust-sources.json` 的 `id` 比较（工作区自有 crate 的 `license` 为空，不计入第三方）：

| 根集合 | 可达 crate | 不在台账 |
| --- | --- | --- |
| M2/M3 运行时（agent-core + agent-tools + plugin-host + plugin-host-adapter） | **170** | **142** |
| Tauri 壳（mode-tauri） | **560** | **485** |
| 根 workspace 并集 | **574** | **499** |

M2/M3 缺口里的代表性新依赖（许可证均为宽松型，逐条可从 `cargo metadata` 复现）：

- 存储/加密：`rusqlite@0.32.1 [MIT]`、`libsqlite3-sys@0.30.1 [MIT]`、`sha2`、`digest`、`ring`；
- HTTP/TLS：`reqwest@0.13.5`、`rustls@0.23`、`hyper`、`tokio`、`tower-http`；
- 平台/工具：`windows-sys@0.52/0.61`、`core-foundation`、`jni`、`walkdir`、`thiserror`；
- 数据类：`webpki-root-certs@1.0.9 [CDLA-Permissive-2.0]`、`icu_*` / `zerovec` 等
  `[Unicode-3.0]`。

未发现 GPL/AGPL/MPL 类互惠许可证条目；**这不等于完成合规审查**——M4 仍要逐条生成
许可证原文快照并人工复核。

复现命令（只读元数据，不读上游实现）：

```bash
cargo metadata --format-version 1 --locked > /tmp/meta.json
node - <<'EOF'
const fs = require("fs");
const meta = JSON.parse(fs.readFileSync("/tmp/meta.json", "utf8"));
const ledger = JSON.parse(fs.readFileSync("third-party/rust-sources.json", "utf8"));
const known = new Set(ledger.components.map((c) => c.id));
const byId = new Map(meta.packages.map((p) => [p.id, p]));
const nodes = new Map(meta.resolve.nodes.map((n) => [n.id, n]));
const roots = meta.packages
  .filter((p) => ["agent-core", "agent-tools", "plugin-host", "plugin-host-adapter"].includes(p.name))
  .map((p) => p.id);
const seen = new Set();
const stack = [...roots];
while (stack.length) {
  const id = stack.pop();
  if (seen.has(id)) continue;
  seen.add(id);
  for (const dep of nodes.get(id)?.deps ?? []) stack.push(dep.pkg);
}
const external = [...seen].map((id) => byId.get(id)).filter((p) => p?.license);
const missing = external.filter((p) => !known.has(p.name));
console.log(`${external.length} external crates, ${missing.length} missing from ledger`);
EOF
```

## 环境限制（本机不能重生成台账）

`node scripts/licenses.mjs notices --strict` 在本机失败：

```text
Error: pnpm -r ls --prod --json --depth Infinity
{"error":{"code":"EMFILE","message":"EMFILE: too many open files, open 'D:\\\\ZCodium\\\\node_modules\\\\argparse\\\\package.json'"}}
```

- `ulimit -n 65535` 在 Git Bash 里实际上限是 **3200**，抬高后仍 EMFILE；
- 这是「本机安装树 + Windows 句柄」限制，不是台账输入错误；
- 权威生成路径是 Linux CI（`licenses-notices.yml`），它会重生成并与已入库台账比对；
- 触发 CI 需要推送（**必须由人**）。本地只能做静态盘点与脚本校验。

## 处置选项

| 选项 | 含义 | 代价 |
| --- | --- | --- |
| **(a) 按「发布目标并集」扩 scope（建议）** | `rust-sources.json` 的 scope 改成「Tauri 壳 + agent 运行时 + CUA helper」三组目标，生成器按目标并集收集；CI 生成 | 台账从 63 增至约 500 条（含平台条件依赖），文件变大但口径完整 |
| (b) 每个产物一份台账 | Tauri 壳 / CLI / CUA helper 分文件 | 无冗余，但生成器与门禁都要支持多份，维护成本高 |
| (c) 等 Rust 组件真正进安装包再扩 | 现在不动，发版前补 | 风险是「发版时才想起」，且届时要同时改生成器、门禁与 NOTICE 模板 |

建议 (a)：与 README 里「保守并集，不声称每个平台都链接」的既有口径一致，也避免
per-artifact 台账在 M5 切发布线时新增分支。

## T2 覆盖门禁（2026-10-18 已实现）

工具：`scripts/rust-ledger-gap.ts`，命令：

```bash
pnpm check:rust-ledger
# 本机无 cargo 时：先生成 metadata 再传文件
node scripts/rust-cargo.ts metadata --format-version 1 --locked > .plan/meta.json
pnpm check:rust-ledger -- --metadata .plan/meta.json
pnpm check:rust-ledger -- --json
pnpm check:rust-ledger -- --strict   # T3 之后接成阻断门禁
```

口径：

- **发布目标** = workspace 全部成员，排除 `tauri-test-manifest` 与 `updater-verify`
  （可用 `--roots a,b` 覆盖）；
- 每个目标取 `cargo metadata` 的 resolve 可达闭包，只统计 `source !== null` 的第三方 crate；
- 覆盖判定是 **`name@version` 精确匹配**（版本漂移单独计数）；工作区自有 crate 不计入；
- 外部 crate 缺 `license` 标识 → `unknownLicense`，`--strict` 同样失败（fail closed）。

**实测（2026-10-18，根 `Cargo.lock`）**：`union: external 558, covered 44, missing 514,
version-drift 31`。与上文「并集 574 / 499」的差异来自两处口径：上文把工作区 crate 计入了
可达集合、且只按 crate 名比较；门禁排除工作区并拒绝版本漂移，因此更严格。

当前**不接 CI**：缺项是 T3 之前的事实，接成阻断会让发布线必红。T3 重生成台账后，
在 `licenses-notices.yml` 里加 `pnpm check:rust-ledger --strict`。



## T1 台账生成器（2026-10-18 已实现，候选未入库）

工具：`scripts/generate-rust-ledger.ts`。它读取两份 `cargo metadata`（根 workspace + 
`crates/mode-cua-ax` 的独立 lock）、两份 `Cargo.lock` 的 checksum 与
`$CARGO_HOME/registry/src` 里的许可证原文，产出：

- `rust-sources.json`（与现有 schema 兼容，新增 `targets`；无许可证文件的条目标
  `reviewRequired: true`）；
- `license-texts/<sha256>.txt`（CRLF 归一后按内容哈希命名，与 notices 生成口径一致）。

```bash
node scripts/rust-cargo.ts metadata --format-version 1 --locked > .plan/root-meta.json
node scripts/rust-cargo.ts metadata --manifest-path crates/mode-cua-ax/Cargo.toml \
  --format-version 1 --locked > .plan/cua-meta.json
pnpm exec tsx scripts/generate-rust-ledger.ts \
  --root-metadata .plan/root-meta.json --cua-metadata .plan/cua-meta.json \
  --out .plan/rust-ledger-candidate
```

**2026-10-18 实测**（输出在 `.plan/rust-ledger-candidate/`，未入库）：577 个组件
（root 14 个目标 + mode-cua-ax）、300 份唯一许可证文本、**47 条按声明标识自动附标准文本**
（`licenseTextSource: "standard"` + `reviewNote`；其中 44 条纯宽松，3 条附文本后仍需人工判断）；
**3 条保留 `reviewRequired`**：`selectors@0.38.0`（已附 MPL-2.0，源码可得性/措辞待人）、
`r-efi@5.3.0` / `6.0.0`（已附 MIT + Apache-2.0，LGPL 选项未附，许可选择待人）；
自校验 root 558/558、CUA 63/63、unknown-license 0；现有 63 条 CUA 条目全部包含。

**为什么候选不直接提交**：

- 剩余 3 条需要人工判断：MPL-2.0 的履约方式与 `r-efi` 的许可选择（见
  `2026-10-18-m4-rust-ledger-review.md`）；
- 台账入库会同时改变 `THIRD-PARTY-NOTICES.md` 的 Rust 段，属发布合规材料，必须走
  Linux CI 重生成 + diff review（T3），不能由巡检轮静默替换。

**生成器已知边界**：只读 registry 源目录（不网络下载）；缺失源码目录时退出 1 并列出
crate（提示先 `cargo fetch`）；git 依赖目前不存在（实测全部是 registry 源）。

## 分阶段任务（M4）

1. **T1 扩 scope 与输入**〔2026-10-18 生成器已实现，见上一节；台账候选待 T3 入库〕：
   `rust-sources.json` 增加 `targets`（`tauri-shell` / `agent-runtime` / `cua-helper`）并把
   生成输入从单一 `mode-cua-ax/Cargo.lock` 改为各目标可达闭包；scope 文案同步。
2. **T2 覆盖门禁**〔2026-10-18 已实现，见上一节〕：对每个 shipped target 的 resolve 图，
   断言所有带 `license` 的 crate 都在台账中；缺一条即失败。当前 report，T3 后接
   `--strict` 到 Linux CI 的 notices job。
3. **T3 重生成与模板**：Linux CI 重生成 `THIRD-PARTY-NOTICES.md` 与 `inventory.json`，
   人工 review diff 后入库；NOTICE 的「Rust crates」段随之覆盖新目标。
4. **T4 npm 侧复核**：M2/M3 没有新增生产 npm 依赖（插件宿主 bundle 用 esbuild，属 dev），
   仍按既有 CI 门禁在依赖树变化时重生成；不需要额外工作。

**验收**：T2 门禁绿；`THIRD-PARTY-NOTICES.md` 的 Rust 段覆盖三个目标；台账 `scope` 与实际
输入一致；本文件列的 142 条 M2/M3 缺口全部有去向（进入台账或明确不发布）。

**必须由人**：推送触发 Linux CI、复核生成 diff、法务措辞最终确认。

## 不在本文件范围

- 许可证条款的法务解释与履约结论；
- npm 依赖的逐包复核（`licenses-notices.yml` 已覆盖生成与新鲜度）；
- 上游衍生代码的处理（见 `2026-10-18-m4-overlap-reduction.md` 与 `clean-room-policy.md`）。
