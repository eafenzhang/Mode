# Spec：语言政策

前置：路线图 `docs/plans/2026-10-09-tauri-rewrite-roadmap.md` M0「收窄技术语言」。
本 spec 定义允许语言、禁入门禁、豁免与收紧机制；门禁脚本为
`scripts/check-language-policy.mjs`，豁免清单为 `scripts/language-policy-allowlist.txt`。
控制器裁定 R2（扫描口径 = `git ls-files` 索引全量，而非计划原文的
`git diff --cached`——后者在 pre-push/CI 接线点会空转）、R3（接线点 =
根 `package.json` 的 `verify:pre-push` + `.github/workflows/release.yml` 的
verify job，本仓无通用 PR CI，且不改 `upstream-audit.yml`）、R7（清单陈旧条目
只告警不失败）优先于计划原文。

## 允许语言

新代码只允许：

- **Rust**
- **TypeScript**
- **HTML**
- **CSS**
- **shell**：仅限既有的 4 个胶水文件——`scripts/count-lines.sh`、
  `scripts/doctor-macos-release-app.sh`、`scripts/prepare-prebuilds.sh`、
  `harness/remote/build.sh`；新增 `.sh` 需 spec 批准（门禁脚本暂不检查 `.sh`，
  靠评审把关）。

## 禁入

非 vendored 路径下新增以下扩展名的文件（大小写不敏感）即门禁失败：

`.py`、`.cs`、`.js`、`.mjs`、`.cjs`

判定口径：对 `git ls-files`（索引全量，裁定 R2）逐文件检查——命中扩展名
且 不属豁免前缀 且 不在允许清单 → 违规，脚本打印违规路径与计数并以退出码 1 失败。

## 豁免（vendored，不检查）

以下路径属 vendored，不参与语言收窄、不被门禁检查：

- `.agents/skills/**`
- `third-party/**`

依据：`third-party/copied-components.json` 台账（第三方复制件的来源与许可记录）
与 `.gitattributes` 的 `linguist-vendored` 标记（`.agents/skills/**`，含其自带的
C#/Python/XSD 等原生语言文件），使其不计入 GitHub 语言统计。

## 豁免机制（允许清单）

确需保留的现存禁入扩展名文件，列入 `scripts/language-policy-allowlist.txt`：

- 一行一个仓库相对路径（`/` 分隔），`#` 起始行为注释，空行忽略；
- **加入清单即视为「spec 批准」**，随提交走评审，不允许静默新增；
- 清单只减不增：日常改动只应删除行。

## 收紧机制

文件被删除（从 `git ls-files` 消失）后，应同步删除其在允许清单中的行；
门禁对陈旧行只打印 `warn: 语言政策清单陈旧条目（文件已删除，建议同步移除）：…`，
**不影响退出码**（裁定 R7）——只有新增违规才失败。

## 运行点

- 本地：`pnpm verify:pre-push`（根 `package.json`，lint 之后、架构检查之前调用
  `node scripts/check-language-policy.mjs`）；
- CI：`.github/workflows/release.yml` 的 verify job，`pnpm lint` 之后的
  `Language policy gate` step（裁定 R3）。

## 验收场景

1. **新增违规 → fail**：在仓库根写入 `foo.py` 并 `git add foo.py`，运行
   `node scripts/check-language-policy.mjs` → 退出码 1，违规列表含 `foo.py`。
2. **清理 → pass**：`git rm --cached -f foo.py` 并删除该文件后重跑 → 退出码 0。
3. **清单陈旧行 → warn 不 fail**：向允许清单临时追加一个不存在的路径后重跑 →
   打印陈旧告警、退出码仍 0；对真实树运行最终态退出 0。

门禁随 `verify:pre-push` 与 release CI 生效，因此以上场景也是每次推送与发版的
回归基线。
