# M4 Rust 台账：reviewRequired 分类与收尾报告（2026-10-18）

> 输入：`.plan/rust-ledger-candidate/rust-sources.json`（T1 生成器候选，未入库）中的
> `reviewRequired` 条目、根 `cargo metadata` 的可达图、`scripts/license-texts/` 现有文本。
> 本文件是**分类与待办**，不是法务结论；最终判断由人完成。

## 结论

- 候选台账 577 条中有 **47 条**在 registry 包内**没有根级许可证文件**；它们的 license
  标识来自 `Cargo.toml`。
- 生成器已为其中 **47 条**按声明标识自动附上仓库标准文本（`licenseTextSource: "standard"`
  + `reviewNote`）；**3 条保留 `reviewRequired`**：`selectors@0.38.0`（已附 MPL-2.0 文本，
  源码可得性/措辞待人）、`r-efi@5.3.0` / `6.0.0`（已附 MIT + Apache-2.0 文本，LGPL 选项
  未附，许可选择待人）。
- 47 条中 **42 条**可从发布目标到达，**5 条只在 CUA helper 图**（`napi` 家族：
  napi / napi-build / napi-derive / napi-derive-backend / napi-sys）。
- `scripts/license-texts/` 现有 53 份文本，MIT/Apache/BSD/Zlib/MPL/LGPL 体裁均已存在，
  补文本不需要新下载。

## 需要真正判断的三处

### 1. `selectors@0.38.0`（MPL-2.0，唯一弱互惠项）

依赖链（从发布目标出发，实测）：

```text
mode-tauri -> tauri@2.12.4 -> tauri-utils@2.10.2 -> dom_query@0.28.0 -> selectors@0.38.0
```

同时被 `deeplink` / `platform` / `tray` / `updater` 的图包含。MPL-2.0 是**文件级**弱互惠：
分发时需要保留许可证与源码获取方式（MPL 覆盖的文件），不是整库传染。需要人确认：

- notices 是否附 MPL-2.0 全文（现有语料已有）；
- 以何种方式提供 MPL 覆盖文件的源码（发行物附带/仓库 tag 指引），与 `NOTICE` 措辞一致；
- 是否存在条款要求的修改披露。

### 2. `r-efi@5.3.0` / `r-efi@6.0.0`（MIT OR Apache-2.0 OR LGPL-2.1-or-later）

链：`mode-tauri -> protocol -> schemars -> uuid -> getrandom -> r-efi`。
三选一许可能用 MIT 或 Apache 路径满足；需要在台账/通知里**明确选择**，不要同时列 LGPL
造成「已选择 LGPL」的误读。人确认选择后，生成器按选择附文本即可。

### 3. 42 条纯宽松项

包括 `jni*`、`objc2*`（Zlib/Apache/MIT）、`webview2-com*`、`winapi-*`、
`alloc-stdlib`（BSD-3-Clause）、`dispatch2`、`dlopen2*`、`tauri-plugin` 等。
处理方式：按声明的 SPDX 标识附加现有标准文本，`reviewRequired` 改为说明性字段
（例如 `licenseTextSource: "standard"`）。人工只需抽样确认映射，不必逐条读文本。

**2026-10-18 已落地**：生成器 `indexStandardLicenseTexts` 按正文特征建立
标识符→文本索引，对全部 47 条无包内文件的条目附文本（`licenseTextSource: "standard"`）；
MPL 照附、LGPL 选项不附。`reviewRequired` 从 47 降到 **3**，且 3 条都已有对应许可文本，
人工只需决定 `r-efi` 的许可选择与 `selectors` 的 MPL 履约措辞。

## 建议的 T1 收尾口径

1. 〔已完成〕生成器增加「标准文本附加」：从现有 `scripts/license-texts/` 按标识选择文本
   （MIT/Apache-2.0/BSD-3-Clause/Zlib/MPL-2.0）；LGPL 选项不自动附，保留人工选择；
2. 对 `r-efi` 记录选择（建议 MIT），对 `selectors` 记录 MPL 处理方式；
3. 复核后 `reviewRequired` 数量应降为 **0**，或在条目上保留 `reviewNote` 说明
   （而不是静默丢弃标记）；
4. 之后进入 T3：Linux CI 重生成 + `THIRD-PARTY-NOTICES.md` diff review。

## 复现

```bash
node -e "const d=require('./.plan/rust-ledger-candidate/rust-sources.json');
console.log(d.components.filter(c=>c.reviewRequired).length)"          # 47
pnpm exec tsx scripts/generate-rust-ledger.ts \
  --root-metadata .plan/cargo-meta-full.json --cua-metadata .plan/cua-meta.json \
  --out .plan/rust-ledger-candidate
```

依赖链与可达性来自根 `Cargo.lock` 的 `cargo metadata`；分类只看 license 标识与文件存在性，
没有读取上游实现。

## 不在本文件范围

- MPL/LGPL 条款的法务解释与履约方案；
- notices 的最终措辞（T3 人工 review）；
- npm 依赖侧（`licenses-notices.yml` 已覆盖）。
