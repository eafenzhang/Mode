# 发布清单（Release Checklist）

适用范围：本仓库的 Tauri 桌面线（**Windows x64**）发布与回滚。所有命令默认在仓库根目录执行。

> 状态标记（读每一条前先看它属于哪一类）：
>
> - **【现在可执行】**：当前检出可直接跑。
> - **【Task 1 完成前不可执行】**：依赖更新链（签名密钥、`latest.json`、真更新演练），见
>   `docs/plans/2026-10-17-m5-release-cutover.md` 的 Task 1。**Task 1 已完成**。
> - **【Task 2 完成前不可执行】**：依赖发布线切换（Tauri 产物进 Release、`publish` 依赖 `tauri`
>   job），见同一计划的 Task 2。**Task 2 已完成**（本清单已按切换后的流水线更新）。
> - **【待确认】**：仓库里没有可引用的实现或原始输出；后面写明「什么能把它定下来」。

依据：`.github/workflows/release.yml`、`apps/desktop-tauri/package.json`、
`apps/desktop-tauri/src-tauri/tauri.conf.json`、`docs/specs/tauri-renderer-host.md` §13.4/§20、
`docs/specs/tauri-platform-commands.md` §8、`docs/plans/2026-10-17-m5-release-cutover-input.md`。

---

## 1. 构建

### 1.1 产物名契约

对外发布名固定为：

```text
Mode-<版本>-<平台>-<架构>.<扩展名>
```

Windows x64 的 Tauri 产物是：

```text
Mode-<版本>-win-x64.exe
```

CI 在 `Collect installer and record size` 步骤里把 bundler 的本机名改成契约名
（`.github/workflows/release.yml` 的 `out="dist/tauri/Mode-${VERSION}-${TARGET}-${ARCH}.${EXT}"`）。
**本机 bundler 的原始名不是发布名**：`tauri build` 产出的是
`target/x86_64-pc-windows-msvc/release/bundle/nsis/Mode_<版本>_x64-setup.exe`（下划线，且带
setup 后缀）。不要直接把 bundler 原始名上传到 Release。

旧 Electron 线的产物名同样是 `Mode-<版本>-win-x64.exe`
（`packages/desktop/electron-builder.config.js` 的 `buildDesktopArtifactName("win")`）。
这正是 DR-M5-1 要求「同一次 run 不能既传 Electron 又传 Tauri 到同一个 release」的原因。

### 1.2 两条前端构建：产品页与验收台

| 用途 | 命令 | 产物 | 说明 |
| --- | --- | --- | --- |
| **产品页（发布路径）** | `pnpm --filter @mode/desktop-tauri run build:frontend-for-tauri` | `apps/desktop-tauri/dist/index.html` + `dist/assets/**` | 脚本是 `build-renderer-for-tauri.ts --no-bench`：先构建 `packages/desktop` 的 renderer，再拷到 `dist/` 根；**不含** `dev/` |
| **验收台（开发/诊断）** | `pnpm --filter @mode/desktop-tauri run build:frontend` | `apps/desktop-tauri/dist/dev/index.html` | 就是 `vite build`（`apps/desktop-tauri/vite.config.ts` 的 `outDir: "../dist/dev"`）；**不是产品页**，只能配合 `MODE_TAURI_SHELL_ENTRY=dev/index.html` 使用 |
| **打包输入** | `pnpm --filter @mode/desktop-tauri run build:package-inputs` | `dist/` + `dist-host-sidecar/ship/**` | = `build:frontend-for-tauri` + `build-host-sidecar.ts --stage`（默认 `pruned` 档）；`tauri build` 的 `beforeBuildCommand` 自动跑它 |

约束（全部可核对）：

- `tauri.conf.json` 的 `build.beforeBuildCommand` 是
  `pnpm --dir ../.. --filter @mode/desktop-tauri run build:package-inputs`，`frontendDist` 是
  `../dist`。所以正常 `tauri build` 不需要再手工构建前端。
- `build-renderer-for-tauri.ts` 在装配前会 `rmSync(distDir)`，旧的 `dist/dev/` 不会被带进发布包。
- **不要把 `build:frontend` 当成发布构建**：它只产出 `dist/dev/`，没有 `dist/index.html`。
  用错档的症状是「编译能过，但包里的产品页不是产品页」；`verify-installed.ts` 的
  「产品页资产名在壳 exe 里」与「验收台不在 dist/ 里」两条断言会挡住它。
- 验收台多占体积并暴露内部命令面，发布路径用 `--no-bench`（DR-T3-2）。

### 1.3 版本注入

- `apps/desktop-tauri/src-tauri/tauri.conf.json` 的 `version` 是占位值 `0.0.1`，**不要**为了发版改它。
- 版本在构建命令上注入，来源是 release 流程算出的 `<版本>`（tag 去掉 `v`）：

  ```bash
  pnpm --filter @mode/desktop-tauri exec tauri build \
    --target x86_64-pc-windows-msvc \
    --bundles nsis \
    --config '{"version":"<版本>"}'
  ```

  CI 的原样命令在 `.github/workflows/release.yml` 的 `Build Tauri bundles` 步骤
  （`--config "{\"version\":\"${{ needs.release.outputs.version }}\"}"`）。
- 旧 Electron 线用的 `MODE_APP_VERSION` **不适用于 Tauri 线**；Tauri 只认 `--config`。

### 1.4 本机复现构建（【现在可执行】）

```bash
pnpm install --frozen-lockfile

# 产品页 + Host sidecar 发运集（tauri build 也会自动跑，这里显式跑一遍便于排错）
pnpm --filter @mode/desktop-tauri run build:package-inputs

# NSIS 安装包（Windows x64）。签名环境变量只在当前 shell，不落仓库。
export TAURI_SIGNING_PRIVATE_KEY="$HOME/.mode-tauri-updater/mode-updater.key"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$(cat "$HOME/.mode-tauri-updater/password.txt")"

pnpm --filter @mode/desktop-tauri exec tauri build \
  --target x86_64-pc-windows-msvc \
  --bundles nsis \
  --config '{"version":"<版本>"}'

# 改成发布契约名，并落 sha256 与体积（与 CI 同一脚本）
mkdir -p dist/tauri
cp "target/x86_64-pc-windows-msvc/release/bundle/nsis/Mode_<版本>_x64-setup.exe" \
   "dist/tauri/Mode-<版本>-win-x64.exe"
# .sig 必须与改名后的安装包同批；latest.json 的 signature 字段直接吃它
cp "target/x86_64-pc-windows-msvc/release/bundle/nsis/Mode_<版本>_x64-setup.exe.sig" \
   "dist/tauri/Mode-<版本>-win-x64.exe.sig"
# 与安装包同批的前端入口快照：verify-installed.ts 用它比对「产品页资产名在壳 exe 里」
cp "apps/desktop-tauri/dist/index.html" \
   "dist/tauri/Mode-<版本>-win-x64.exe.index.html"
node scripts/tauri-bundle-report.ts "dist/tauri/Mode-<版本>-win-x64.exe"

# 生成 updater 清单（Task 1 实测：tauri build 本身不产出 latest.json，只产出 .sig）
# `--notes` 不能留空：updater 会把它展示给用户；CI 里指向本次 Release 的 tag。
node scripts/tauri-update-manifest.ts \
  --version "<版本>" \
  --artifact "dist/tauri/Mode-<版本>-win-x64.exe" \
  --signature "dist/tauri/Mode-<版本>-win-x64.exe.sig" \
  --url "https://github.com/eafenzhang/Mode/releases/download/v<版本>/Mode-<版本>-win-x64.exe" \
  --notes "Release notes: https://github.com/eafenzhang/Mode/releases/tag/v<版本>" \
  --output "dist/tauri/latest.json"
```

> **本机工具链注记**：本机没有 MSVC Build Tools，`node scripts/rust-cargo.ts` 走
> `x86_64-pc-windows-gnu`。M5 Task 1 的演练就是用 gnu 宿主工具链构建的
> （`RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-gnu` + `~/.rust-tools/llvm-mingw-*` 的
> `gcc`/`dlltool`/`windres`）；CI 仍用 msvc。两者的 NSIS 安装包格式与 updater 路径相同，
> 但 **msvc 产物没有在本机重跑过**，见 2.2 V11 的边界说明。

要求：Rust 稳定工具链含 `x86_64-pc-windows-msvc` 目标；Node 版本以 `mise.toml` 为准
（CI 用 Node 24）。`scripts/tauri-bundle-report.ts` 会写 `<安装包>.sha256`、`size.txt`，
并在 GitHub Actions 里追加 job summary。

### 1.5 体积口径（不阻断发布）

决定：**体积不再是发布阻断项**（DR-M5-2b，2026-10-17）。但测量纪律不变：

- **门禁脸 = NSIS 安装包字节**（`statSync(安装包).size`，MiB = 字节 / 1024²）。
  每个数字都必须写明它测在哪张脸上。
- 当前实测（M3，出处 `docs/specs/tauri-renderer-host.md` §13.4 与
  `docs/specs/tauri-platform-commands.md` §8.2）：

  | 数字 | 值 | 测在哪张脸 |
  | --- | --- | --- |
  | 核心（壳 exe + UI 资产 + Host bundle + `config/`） | 23.29 MiB | NSIS 安装包字节 |
  | 产品档总包（含随包 Node + 剪枝后 externals） | 51.50 MiB | NSIS 安装包字节 |
  | 运行时 + 服务面增量（总包 − 核心） | 28.22 MiB | NSIS 安装包字节（差值） |
  | 剪枝收益（原始） | 30.41 MiB | 未压缩文件字节和（诊断，不是门禁口径） |
  | 剪枝收益（压缩后） | 2.80 MiB | NSIS 安装包字节（差值） |

- **不设新的阈值**。剪枝报告必须同时给出原始与压缩收益（只报原始值会把 30 MiB 的优化
  在门禁上读成 2.80 MiB 的大改动）。
- 复算命令（本机出三个包，不进 CI）：
  `node --import tsx apps/desktop-tauri/scripts/measure-installer-budgets.ts`。
- 重新开启体积门禁的条件：弱网安装或分发渠道提出硬限制时，重开 DR-M5-2b；届时优先的杠杆是
  「别把插件/技能负载打进包」，不是重写后端。

#### 1.5.1 **仍未解决的口径问题（待人拍板，M5 不自行改数）**

同一个「≤30MB」在仓库里指向两张不同的脸，这是 **§13.4 的数字矛盾**，M5 Task 3 只登记、不裁决：

- **读法 A（核心面）**：路线图 M5 写的是「初装 ≤30MB **核心** + sidecar 按需」
  （`docs/plans/2026-10-09-tauri-rewrite-roadmap.md` 第 241 行），spec §13.4 的 (a) 实测
  **23.29 MiB ≤ 30MB**，按这张脸是达标的。
- **读法 B（整包面）**：M5 计划 `docs/plans/2026-10-17-m5-release-cutover.md` 第 33 行写的是
  「原门禁『≤30MB』指的是**整包**」；当前产品档总包实测 **51.50 MiB**，按这张脸不达标。

两者还牵涉一个产品形态问题：路线图的「sidecar 按需」与当前「随包 Node + externals」不一致。
**没有新的阈值**（DR-M5-2b 已放开阻断），也**没有**在本任务里挑一个数字。人需要给出：
① 官方体积口径是哪张脸；② sidecar 是否必须改成按需下载。给出之前，发布按「只记录、不阻断」执行。
交叉引用：`docs/specs/tauri-renderer-host.md` §13.4.1、`docs/specs/tauri-platform-commands.md` §8.2。

### 1.6 平台范围：Windows-only（登记偏差）

- **决定**：M5 只发布 Windows x64 的 Tauri 版（DR-M5-2，人，2026-10-17）。
- **偏差**：路线图 M5 验收写的是「三平台 GA」；本阶段不满足该条，且**不改路线图原文**。
- **原因**：Ruling 20 把 macOS/Linux 移出 M3；两条腿从未出过真产物、从未验证。CI 的
  `tauri` job 里 mac/linux 仍是 `continue-on-error`（`.github/workflows/release.yml` 的
  `continue-on-error: ${{ matrix.target != 'win' }}`）。
- **代价**：Windows 之外的用户在切换后没有 Tauri 版；旧 Electron 线的产物与回滚能力必须保留
  （见第 5 节）。macOS/Linux 的体积、冷启动、webview 路径全部**未验**，不能从 Windows 结论外推。
- **解冻条件**：先收口 `docs/specs/tauri-platform-commands.md` §8.3 的两条已知风险——
  Linux 的 `webkit2gtk`/`libsoup3` 包名与 AppImage 的 FUSE 绕过；macOS 需确认产出的是
  `aarch64-apple-darwin`。收口后重新开一次决策，再撤销本条偏差。

---

## 2. 验证

### 2.1 自动验证（CI 实际会跑的）

**`release.yml` 的 `verify` job（Linux runner）** 原样跑：

```bash
pnpm install --frozen-lockfile
pnpm typecheck
pnpm lint
node scripts/check-language-policy.mjs
pnpm --filter "@mode/bootstrap..." build
pnpm --filter "@mode/core..." build
pnpm --filter @mode/shared test
pnpm --filter @mode/services test
pnpm --filter @mode/server test
pnpm --filter @mode/ui test
pnpm --filter @mode/cua test
pnpm --filter @mode/core test
pnpm exec tsx --test packages/desktop/tests/*.test.mjs packages/desktop/tests/*.test.mts
pnpm --filter @mode/desktop-tauri test
```

其中 `@mode/desktop-tauri test` 会跑 `apps/desktop-tauri/test/*.test.mts`：
`portRoundTrip`、`portMessagePath`、`hostChannelStdio`、`windowModeInjection`、`nativeDeps`、
`realServices`、`shipSet` 等自动用例；四个 opt-in 用例默认 **skip**（见 2.2）。

**`release.yml` 的 `desktop` job（Windows runner，仅 `matrix.target == 'win'`）** 跑下列步骤。
**它是诊断/回滚构建，不是发布线的一部分**：`continue-on-error: true`，失败会在 UI 里显示，
但不会把 run 判红，也不进 `publish` 的 needs。

```bash
pnpm typecheck
pnpm build:cua-helper
cargo test --manifest-path crates/mode-cua-ax/Cargo.toml
cargo clippy --manifest-path crates/mode-cua-ax/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path crates/browser-import-helper/Cargo.toml
cargo clippy --manifest-path crates/browser-import-helper/Cargo.toml --all-targets -- -D warnings
node packages/desktop/scripts/build-windows-browser-import-helper.mjs
pnpm --filter @mode/desktop prepare:browser-import-helper
node --test packages/desktop/tests/browser-import-helper-contract.test.mjs
pnpm --filter @mode/desktop-tauri run build:frontend-for-tauri
cargo test -p platform -p protocol -p updater -p deeplink -p sidecar-host
cargo clippy -p platform -p protocol -p mode-tauri -p updater -p deeplink -p tray -p sidecar-host \
  --all-targets -- -D warnings
node scripts/generate-tauri-protocol.mjs --check
```

**`release.yml` 的 `tauri` job** 构建 NSIS 安装包、改名、跑
`node scripts/tauri-bundle-report.ts`（sha256 + 体积留痕）。M5 Task 2 起：

- **Windows 腿是唯一的 Release 产物来源**：签名（`TAURI_SIGNING_PRIVATE_KEY` /
  `_PASSWORD`）→ 安装包 + `.sig` → `node scripts/tauri-update-manifest.ts` 生成
  `latest.json`（`--notes` 指向本次 tag，不再留空）→ `gh release upload` 三个文件 →
  上传 `release-checksums-tauri-win-x64` 片段。Windows 腿还会跑
  `cargo clippy -p mode-tauri --features port-transport-harness,host-roundtrip,product-ui-harness --bins -- -D warnings`
  （§20 第 4 条的 required-features harness 覆盖）。
- **macOS / Linux 腿**仍只上传 14 天期的 workflow artifact，`continue-on-error`；
  它们不产出校验片段。job 级 `continue-on-error` 会把它们的失败在 `needs` 里归一为
  `success`，所以不挡 Windows-only 发布（见下方「重要」）。
- **旧 Electron `desktop` job 仍构建**（保留回滚能力），但只上传
  `electron-desktop-*` / `electron-checksums-*` workflow artifact，不再写 Release；
  它已声明 `continue-on-error: true`——诊断构建失败不会把整次 run 判红，也不挡 `publish`。

> **M5 Task 1/2 之后的前置项**：`tauri.conf.json` 已开 `createUpdaterArtifacts` 且 pubkey 非空。
> 如果 CI secrets 里没有 `TAURI_SIGNING_PRIVATE_KEY` / `_PASSWORD`，`Build Tauri bundles`
> 会以非 0 退出（fail closed，实测报错见 3.3），不会留下可发布的未签名包。先按 3.1.1
> 创建 secret，再推 `main`。

本地推送前还必须跑（CI 不完整覆盖架构检查）：

```bash
pnpm typecheck
pnpm lint
pnpm verify:pre-push
```

> **重要（M5 Task 3 之后）**：`publish` 与 `checksums` 现在**直接断言**
> `needs.tauri.result == 'success'`。Windows 腿没有 `continue-on-error`，所以它失败时
> 结果是 `failure`，`publish` 不会公开 release——**不再**依赖「checksums 缺片段」这条
> 间接证明。macOS/Linux 腿仍有 job 级 `continue-on-error`；GitHub 会把这种腿在
> `needs` 结果里归一为 `success`（`actions/runner#1161`，UI 仍显示该腿失败），所以它们
> 失败不会挡 Windows-only 发布（DR-M5-2）。`checksums` 仍保留：它证明安装包、`.sig` 与
> `latest.json` 确实进了本次 Release。
> 同一次 run 里 Electron 的 `desktop` job 不再上传 Release，所以两条线不会互覆。

**三指标与门禁（路线图 M5 第 241 行原文）**：

| 指标 | 路线图原文 | 门禁落点 | 今天的状态 | 何时变成阻断 |
| --- | --- | --- | --- | --- |
| 重合度 | `pnpm verify:self-developed`（重合度 0） | `release.yml` 的 `verify` job：`Upstream overlap metric (roadmap M5; non-blocking until M4 tool exists)` | **已接线、不阻断**：`scripts/upstream-overlap-check.mjs` 缺失时打印醒目告警并 `exit 0`（缺失工具由 M4 交付） | **脚本出现即自动阻断**：`scripts/upstream-overlap-check.mjs` 存在时，步骤跑 `pnpm verify:self-developed`，失败即让 `verify` 红。届时删除缺失分支（注释里写死的重开条件） |
| 语言 | `scripts/check-language-policy.mjs`（语言） | `verify` job：`Language policy metric (roadmap M5)`；`pnpm verify:pre-push` 也跑 | **今天阻断**（脚本存在且通过） | 已生效 |
| 体积 | CI 记录安装包体积（初装 ≤30MB 核心 + sidecar 按需） | `tauri` Windows 腿：`Collect installer and record size` + `Record installer size metric (roadmap M5; NSIS installer face, recorded only)` | **只记录、不设阈值**（DR-M5-2b）；记录失败仍会让 `tauri` 红 | **当前不因数值阻断**。只有人重开 DR-M5-2b 并给出阈值时才会变成数值门禁；不要自编数字 |

> 体积的脸必须写死：CI 的 `size.txt` 是**产品档总包**（core + 随包 Node/externals，当前
> 51.50 MiB）在 **NSIS 安装包字节**这张脸上；路线图 M5 的「≤30MB 核心」在同一张脸上是
> **23.29 MiB，已达标**（§13.4），由本机 `measure-installer-budgets.ts` 测，不在 CI 里
> 重复出三个包。别把两者混读。

### 2.2 人工验证（CI 做不到，发布前逐项勾）

这些项要么要真 webview、要么要真安装器、要么要真实指针/肉眼。**漏跑 = 路径静默坏掉而流程说全绿。**
opt-in 用例共用固定的 sink 端口 `127.0.0.1:5177`，**必须一个一个跑**，不要放在同一条 `--test`
命令里并发。全部在仓库根、Windows 本机执行。

- [ ] **V1. 真 webview 端口往返（成功 + 失败两条路径）**

  ```bash
  MODE_TAURI_WEBVIEW_ROUNDTRIP=1 node --import tsx --test apps/desktop-tauri/test/webviewRoundTrip.test.mts
  ```

  判据：成功路径 `status: "ok"`、`gateOpened: true`、`portIsRealMessagePort: true`；
  失败路径 `failedPhase: "preparing_host_storage"` 且 `errorCode: "sql_failed"`。
  出处：`docs/specs/tauri-renderer-host.md` §5、§10.8。

- [ ] **V2. 真产品 UI 挂载**

  ```bash
  MODE_TAURI_PRODUCT_UI=1 node --import tsx --test apps/desktop-tauri/test/productUiMount.test.mts
  ```

  判据：`mounted === true`、`acceptedFrames > 0`、`pumpFramesGrewAfterReady === true`、
  `uiText` 不含 `not_implemented_until_m3`；负向对照里 `mounted === false`。
  出处：`docs/specs/tauri-renderer-host.md` §5、§10.9/§10.11。

- [ ] **V3. 自绘标题栏无边框与窗口控制接线**

  ```bash
  MODE_TAURI_WINDOW_CONTROLS=1 node --import tsx --test apps/desktop-tauri/test/windowControls.test.mts
  ```

  判据：harness 报告 `decorations_expected=false` 且 `decorations_actual=false`；最大化切换读回
  `isMaximized: true` 再切回 `false`；最小化成功；最后关窗后窗口销毁、进程退出、不留 Host。
  敏感性对照里「有系统标题栏」的窗口必须读到 `true` 且 `frameDeltaY >= 20`。
  注意：这条用例**不**覆盖真鼠标拖拽与最小化/最大化的视觉结果（见 V6）。

- [ ] **V4. 单实例与深链转发**

  ```bash
  MODE_TAURI_SINGLE_INSTANCE=1 node --import tsx --test apps/desktop-tauri/test/singleInstance.test.mts
  ```

  判据：第二个进程很快以退出码 0 结束，且它的 stdout/stderr 不含 `host startupId=`；
  进程表里 `host-standalone` 的 `node.exe` **正好 1 个**；已运行实例的页面收到
  `mode://oauth/callback?state=single-instance-forward`。

- [ ] **V5. 生产 Host 端口往返（本地复跑；CI 也跑）**

  ```bash
  node --import tsx --test apps/desktop-tauri/test/portRoundTrip.test.mts
  ```

  判据：全部子项通过——真 Host 的 `Initialize` + `host.describe` 真 RPC 往返、未知命令显式报错、
  未知订阅不杀 Host、非 RPC 帧被丢弃而不崩、数据隔离生效且真实 `~/.mode` 未变。
  这条在 CI 的 `@mode/desktop-tauri test` 里也跑；人工这项是发布前的本地重复确认。

- [ ] **V6. 真鼠标拖拽与最小化/最大化/关闭的视觉结果**

  ```bash
  pnpm --filter @mode/desktop-tauri run build:frontend-for-tauri
  MODE_DATA_BASE_DIR="<一个空的临时目录>" node scripts/rust-cargo.ts run -p mode-tauri
  ```

  逐项目视确认：
  - 按住自绘标题栏的空白区域拖动，窗口**真的移动**；顶部区域也可拖。
  - 点最小化：窗口真的收到任务栏（不是无反应）。
  - 点最大化：窗口真的铺满工作区，图标切换为「还原」；再点真的还原。
  - 点关闭：窗口消失，进程退出；任务管理器里没有残留的 `node.exe`（Host）。
  自动化只覆盖「无边框 + 可拖区域存在 + 按钮打到壳」，**不覆盖真实指针与视觉结果**
  （`windowControls.test.mts` 文件头「覆盖边界」）。

- [ ] **V7. 安装后的 smoke：install → reinstall → run（隔离数据根）**

  ```bash
  node --import tsx apps/desktop-tauri/scripts/verify-installed.ts \
    --installer "dist/tauri/Mode-<版本>-win-x64.exe" \
    --work-dir "<空的临时目录>" --install-dir "<空的临时目录>\install" --keep
  ```

  判据：结论 `22/22 条通过`，且至少包含——覆盖重装后主 exe 与安装包内产物同字节；
  安装目录里有 `host-sidecar/index.mjs`、`node/node.exe`、
  `config/provider/mode-builtin.json`；产品页资产名在壳 exe 里且验收台不在安装目录；
  Host 报 `ready`、无 `startup_failed`；真实写入落在隔离数据根、真实 `~/.mode` 未变；
  随包 Node + sidecar 真开一个 pty 并跑通 `terminal` 频道；退出后无残留 `node.exe`。
  注意：传入的必须是**产品档**安装包（`build:package-inputs` 的 pruned 资源），不是
  `measure-installer-budgets.ts` 出的 `core`/`unpruned` 诊断包。安装包旁边应有一份
  **同批**的 `<安装包>.index.html` 快照（1.4 的 `cp` 会生成；`measure-installer-budgets.ts`
  也会为它出的包生成）。没有快照时脚本会退回本机当前的 `dist/index.html` 并在输出里标注
  「可能与这个包不同批」——那会产生假红，不能当作产物有问题。

- [ ] **V8. 安装后的产品 UI 目视运行**

  用 V7 装出来的 `ModeTauri.exe` 启动一次（数据根继续隔离）：

  ```bash
  MODE_DATA_BASE_DIR="<一个空的临时目录>" "<安装目录>\ModeTauri.exe"
  ```

  确认：产品 UI 真的渲染出来（不是启动/失败屏），窗口可交互；关闭后无残留 `node.exe`。
  `verify-installed.ts` 用 stderr 判据证明 Host 与平台面通了，但**不证明画面**。

- [ ] **V9. 冷启动测量**

  ```bash
  node --import tsx apps/desktop-tauri/scripts/measure-cold-start.ts --release "<安装目录>\ModeTauri.exe"
  ```

  判据：7/7 有效样本，不出现 `startup_failed`；中位数在 §13.5 的实测值量级
  （release 约 `T_ready` 825 ms / `T_surface` 962 ms，首样本约 1.1 s；debug 对照约 1030/1359 ms）。
  这是报告项，不是硬阈值；数字进发布记录，并注明测在哪张脸/哪台机器上。

- [ ] **V10. opt-in harness 的单独 clippy**

  ```bash
  node scripts/rust-cargo.ts clippy \
    --features port-transport-harness,host-roundtrip,product-ui-harness \
    --bins -p mode-tauri -- -D warnings
  ```

  判据：退出码 0。`--all-targets` 覆盖不到 `required-features` 门住的三个 harness bin
  （Host spec §20 第 4 条），所以这条必须单独跑。**M5 Task 3 起 CI 的 `tauri` Windows
  腿也跑同一条命令**；这条人工项保留为发布前的本机重复确认。

- [x] **V11. 旧版 → 新版真更新演练（M5 Task 1 已跑通；每次发版前仍需重跑）**

  **本任务的交付与实测结论**

  - `plugins.updater.pubkey` 已从空串换成真实 minisign 公钥；空串仍然是 fail closed
    （`PublicKey::decode("")` 报错），意图保留。
  - `bundle.createUpdaterArtifacts: true` 是必需的：没有它不产出 `.sig`。
    **`tauri build` 本身不产出 `latest.json`**——Task 1 实测 NSIS 目录里只有
    `Mode_<版本>_x64-setup.exe` 与 `.exe.sig`；清单由
    `node scripts/tauri-update-manifest.ts` 生成。
  - 可执行触发：
    - 产品入口：帮助菜单 →「检查更新」→ `executeDesktopCommand("checkForUpdates")` →
      `updater::command::check_and_install`；
    - 演练入口：`MODE_TAURI_CHECK_FOR_UPDATES=1`（启动时走**同一个函数**）。
      这样做的原因：安装后的自动化演练没有可靠的点按面；两者不是两套更新逻辑。
  - 演练构建用 `--config` 把端点指向本地 HTTPS（`https://127.0.0.1:8443/latest.json`）
    并开 `dangerousAcceptInvalidCerts`（自签证书）。这只用于本地演练，**不进发布产物**。

  **已执行的演练（原始输出在任务报告里）**

  1. `0.0.1` 安装到隔离目录 `C:\Users\Administrator\.mode-tauri-drill\install-0.0.1`，
     `latest.json` 指向 `0.0.2`。
  2. 启动 `0.0.1`（`MODE_DATA_BASE_DIR` 指向隔离目录，`MODE_TAURI_CHECK_FOR_UPDATES=1`）：
     日志依次出现
     `updater check start current=0.0.1` →
     `update_available ... version=0.0.2` →
     `download complete bytes=54030393/54030393` →
     `signature verified bytes=54030393 version=0.0.2`。
  3. 旧进程以退出码 0 结束；安装目录里的 `ModeTauri.exe` 变为 `ProductVersion 0.0.2`；
     重启后的进程 `StartTime` 晚于旧进程退出、`Path` 仍是该隔离安装目录。
     旧进程日志里的 `dataRoot=...\.mode-tauri-drill\data\.mode` 证明隔离数据根生效；
     演练前后 `~/.mode` 的 mtime 未变。
  4. 负向对照：把 served 安装包中间 1 个字节翻转（签名不变）后重装 `0.0.1` 并触发：
     日志出现 `updater download failed error=The signature verification failed`，
     返回 `{"status":"error","phase":"download",...}`，进程继续以 `0.0.1` 运行。
     （把清单里 **untrusted comment** 改一个字符不会失败——那一行不被 minisign 覆盖，
     这不是漏洞；有效对照必须改签名覆盖的字节或产物本身。）

  **每次发版前按下面步骤重跑（Windows 本机）**

  1. 构建并签名版本 N（1.4），安装到隔离目录：
     `Mode_<N>_x64-setup.exe /S /D=<空目录>`。
  2. 准备 N+1 的安装包、`.sig` 与 `latest.json`，把清单与安装包放到一个 HTTPS feed 上；
     N 的构建必须指向该 feed（正式发布用默认的 `/releases/latest/download/latest.json`；
     演练用 `--config` 覆盖端点 + 自签证书）。
  3. 启动 N（建议同时设 `MODE_DATA_BASE_DIR` 与 `MODE_TAURI_CHECK_FOR_UPDATES=1`，
     或在产品里点「帮助 → 检查更新」），确认：下载完成 → 签名校验通过 → 安装器启动 →
     重启 → 安装目录里的 exe `ProductVersion` 为 N+1，且新进程确实在跑。
  4. 负向对照：翻转安装包 1 个字节（或改签名覆盖的字节），确认
     `The signature verification failed`、进程仍为 N。
  5. 清理：停止进程、恢复 `HKCU\Software\Classes\mode` 与卸载项（演练前 `reg export`，
     演练后 `reg import`）、停止本地 feed。

  **边界（不要读成三平台都验过）**：演练在 Windows x64、gnu 本地工具链构建的 NSIS
  安装包上完成；msvc 产物、macOS/Linux 均未演练（DR-M5-2）。

### 2.3 绿 CI 不覆盖什么

明确写下：**CI 全绿不覆盖 V1–V9 与 V11**（V10 的 harness clippy 已进 CI 的 `tauri`
Windows 腿）。原因是 CI 没有真 webview、没有真安装器、没有真实指针/肉眼、
没有可用的更新链；四个 opt-in 用例在 CI 里默认 skip（`MODE_TAURI_*` 环境变量未设置）。
发布前必须逐项勾完 2.2，才能进入第 4 节的发布。

### 2.4 已知开放项（不阻断发布，但必须可见）

§20 逐条处置的权威表在 `docs/specs/tauri-renderer-host.md` §20.2。发布时只需要知道：

- **仍开放（不阻断）**：§20 第 1 条（窗口态发布壳的平台面清单没有稳定落点；从资源管理器
  双击启动看不到降级清单）、第 5 条（`state.disk` 恒为 `[]`，无消费方）、第 6 条
  （Host/壳 `sequence` 单一所有者，合并流未引入）。每条的成本与重开触发见 spec §20.2。
- **已关闭**：第 2、3 条（体积/冷启动已实测）、第 4 条（harness clippy 已进 CI）、
  第 7 条（验收台不进发布产物）、第 8 条（clean-room 违规已登记）。
- **待人口径**：§13.4.1 的体积口径矛盾，见本文件 1.5.1。它不属于 §20 的工程开放项，
  但同样是发布线必须知道的问题。

---

## 3. 签名与清单

### 3.1 两套密钥，不要混为一谈

| 密钥 | 用途 | 存放 | 当前状态 |
| --- | --- | --- | --- |
| **Tauri updater 密钥（minisign）** | 给安装包/更新清单签名，客户端校验 | 私钥只在 CI secrets：`TAURI_SIGNING_PRIVATE_KEY`、`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`；公钥已入库（`tauri.conf.json`） | **已生成**（M5 Task 1）；本机材料在仓库外，见 3.1.1 |
| **Windows Authenticode 代码签名证书** | 消 SmartScreen「未知发布者」 | 证书与密码只在 CI secrets | **未使用**；release notes 明确写「安装包没有代码签名」（`release.yml` 的说明文本） |

纪律：

- 私钥、密码、证书**绝不入库、绝不打印到日志、绝不写进 release notes**。
- 公钥不是秘密，写在 `apps/desktop-tauri/src-tauri/tauri.conf.json` 的
  `plugins.updater.pubkey`。
- **轮换代价**：已发布的旧版本只信任它内置的公钥；私钥一旦轮换，旧版本无法信任新版本。
  所以密钥必须在**第一次对用户正式发布之前**定下来。若私钥泄露，轮换后旧版本用户只能手动安装，
  不能依赖自动更新。
- 生成/签名命令由 Task 1 固定并留下原始输出；本清单不复述未在本仓库验证过的 CLI 参数。

#### 3.1.1 本机密钥材料与 CI secret（人工前置项）

Task 1 用 `tauri signer generate` 生成，**没有写入仓库**：

| 材料 | 本机路径（仓库外） | 去向 |
| --- | --- | --- |
| 私钥 | `C:\Users\Administrator\.mode-tauri-updater\mode-updater.key` | CI secret `TAURI_SIGNING_PRIVATE_KEY` |
| 私钥密码 | `C:\Users\Administrator\.mode-tauri-updater\password.txt` | CI secret `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` |
| 公钥 | `C:\Users\Administrator\.mode-tauri-updater\mode-updater.key.pub` | 已写入 `tauri.conf.json` 的 `plugins.updater.pubkey`（base64 原文） |

**人工要做的一次性动作**（只核对/创建名称，不要在日志里回显值）：

```bash
gh secret set TAURI_SIGNING_PRIVATE_KEY < "$HOME/.mode-tauri-updater/mode-updater.key"
gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD < "$HOME/.mode-tauri-updater/password.txt"
gh secret list --app actions | grep TAURI_SIGNING
```

- 私钥文件与密码文件在创建 secret 后应离线保存（密码管理器或加密介质），不要提交、
  不要贴进 Issue/PR/聊天记录。
- CI 里的 `tauri build` 必须能读到这两个 secret；缺任意一个时构建不会产出可用签名
  （见 3.3 的实测结论）。
- 本地演练用 `TAURI_SIGNING_PRIVATE_KEY=<路径>` 或私钥内容均可；`signer sign` 需要
  `TAURI_SIGNING_PRIVATE_KEY_PATH` 指向文件。

#### 3.1.2 密钥轮换（写清楚代价）

- **为什么不能随便换**：已安装的旧版本只信任它**内置**的 pubkey。直接把 conf 换成新公钥
  后，旧版本会拒绝新版本签名。
- **正常轮换（密钥未泄露）**：
  1. 生成新 keypair；
  2. 发一个**用旧私钥签名**、但 conf 里已含**新公钥**的过渡版本（版本号 +1）；
  3. 之后的版本用新私钥签名。旧用户先升到过渡版，再信任新 key。
- **泄露时**：同上也来不及（攻击者已能签任意版本）。此时必须立即轮换、发布新版本，
  并明确告知用户**手动重装**；旧版本无法靠自动更新获救。
- **何时最便宜**：第一次对用户正式发布之前。当前尚未 GA，轮换成本最低。

### 3.2 `pubkey` 的当前位置与今天的语义

- 文件：`apps/desktop-tauri/src-tauri/tauri.conf.json`
- 字段：`plugins.updater.pubkey`
- **M5 Task 1 起**：真实 minisign 公钥的 base64（`.pub` 文件原文）。
  守卫在 `crates/updater/tests/conf_guard.rs` 的
  `the_app_conf_declares_a_real_minisign_pubkey`：断言非空、base64 可解码、
  且能被 `minisign-verify` 解析。
- 语义：用这把公钥校验每个下载的安装包签名；签名不匹配则更新被拒绝，旧版本继续运行。
- **空串仍然 fail closed**：`PublicKey::decode("")` 报错，任何没有 pubkey 的构建都无法更新。
  这不是历史遗留，而是刻意保留的安全默认；守卫防止有人把 conf 改回空串却以为更新还能用。
- 更新断言时同步改了 `crates/updater/src/lib.rs` 的 `plugin` 文档与
  `docs/specs/tauri-platform-commands.md` §5 的对应句子；旧措辞「因为没有密钥而拒绝一切」
  不再是当前状态，不要照着它排障。

### 3.3 `latest.json` 怎么产生、放哪里

- 端点（常量 + conf 两处，由 `crates/updater` 的单测钉死）：
  `https://github.com/eafenzhang/Mode/releases/latest/download/latest.json`
- 它必须作为 **Release 资产** 上传；今天的 CI 只上传 workflow artifact，**没有这个资产**。
- `latest.json` 的 `version` 必须与本次 tag 去掉 `v` 后一致，`pub_date` 与本次发布同源；
  不要手写第二份版本号。
- **`bundle.createUpdaterArtifacts` = `true`（M5 Task 1 定稿，已提交）**：
  - 不设该键时 schema 默认是 `false`，NSIS 目录里没有 `.sig`；
  - 设为 `true` 且设置签名环境变量后，`tauri build` 产出
    `Mode_<版本>_x64-setup.exe` 与 `Mode_<版本>_x64-setup.exe.sig`；
  - **`tauri build` 本身不产出 `latest.json`**（本机实测：`find target -name latest.json` 为空）。
    它是 `tauri-action` 一类发布工具生成的东西，本仓库用
    `node scripts/tauri-update-manifest.ts` 从 `.sig` 生成。
- `latest.json` 的结构（脚本写出，插件读取）：
  `{version, notes, pub_date, platforms["windows-x86_64"]{signature,url}}`；
  `signature` 是 `.sig` 文件**原文**，`url` 指向本次发布的安装包。
- 签名环境变量缺失时的**实测行为**（`createUpdaterArtifacts: true` + 空 pubkey 之外的正常
  conf + 不设 `TAURI_SIGNING_PRIVATE_KEY`）：`tauri build` 在产出 unsigned 安装包后
  **以非 0 退出**，报
  `A public key has been found, but no private key. Make sure to set TAURI_SIGNING_PRIVATE_KEY environment variable.`
  也就是说它是 fail closed，但会留下一个**没有 `.sig` 的安装包**。
  **人工前置项**：在推送本次提交前先创建 3.1.1 的两个 secret，否则 CI 的 `tauri` job
  会因为缺少私钥而红（这是预期行为，不是新缺陷）。
- **M5 Task 2 起**：`tauri` job 的 Windows 腿把 `latest.json` 与
  `Mode-<版本>-win-x64.exe.sig` 作为 Release 资产上传；`latest.json` 的
  `url` 指向同一 tag 的安装包。`latest*.yml` / `*.blockmap` 属于旧 Electron 线，
  新发布线不再上传它们；两个文件名不同，不冲突。
- **同一次 run 只允许一条线上传安装包**：Electron 的 `desktop` job 仍构建，但只上传
  workflow artifact（`electron-desktop-*`）。这是 DR-M5-1 的 no-clobber 机制：
  产物名逐字相同，但本次 run 里只有 Tauri Windows 腿执行 `gh release upload`。
- **不要把它和 CLI 的 `latest.json` 混淆**：`scripts/build-mode.mjs` 也会在
  `dist/mode/latest.json` 写一份 CLI tarball 的元数据（`scripts/mode-distribution` 用），
  但 `fnos` job 只上传 `.fpk`，不上传这个文件。Release 里的 `latest.json` 只有
  Tauri updater 这一份。

### 3.4 发布前的签名检查（Task 1/2 完成后勾）

- [ ] GitHub Actions secrets 里存在 `TAURI_SIGNING_PRIVATE_KEY` 与
      `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`（只核对名称，不读取/打印值）。
- [ ] `tauri.conf.json` 的 `plugins.updater.pubkey` 非空，且与 CI 使用的私钥配对；
      `crates/updater` 的空串守卫已按 Task 1 更新。
- [ ] `bundle.createUpdaterArtifacts` 为 `true`，且构建确实产出了 `.exe.sig`
      （不是「构建通过」就当作签名成功）。
- [ ] Release 资产里有 `latest.json` 与安装包对应的 `.sig`（M5 Task 2 起由
      `tauri` Windows 腿自动上传；要核对资产名，不能只看 CI 绿）。
- [ ] `latest.json` 的 `version` 等于 tag 去掉 `v`；`pub_date` 存在；平台键是
      `windows-x86_64`；条目指向本次发布的 `Mode-<版本>-win-x64.exe`。
- [ ] **没有**把 `latest.json` 与旧线的 `latest*.yml` 当成同一个资产（两个文件名不同，
      上传时不要互相 `--clobber`）。
- [ ] 负向校验：篡改安装包字节后更新被拒绝（V11 第 4 步；只改清单的 untrusted comment
      不足以构成负向对照）。
- [ ] 私钥轮换预案已演练到「知道怎么换、知道旧用户会怎样」，且没有把任何私钥材料提交进仓库。

---

## 4. 发布

### 4.1 谁执行推送

- **推送由人执行，agent 不执行 `git push`（含 `git push --tags`）。**
- agent 的职责止于「本地提交 + 本地验证通过」并报告未推送的提交；推不推、何时推由人决定。
- 人推 `main` 后，workflow 的 bot 会在构建前创建并推送 tag
  （`release.yml` 的 `Create tag and draft release`），这不是 agent 在推送。

### 4.2 推送 `main` 自动发版（`.github/workflows/release.yml`）

- 触发：`push` 到 `main`。
- 版本推导（原样引用）：只认正式 tag `^v[0-9]+\.[0-9]+\.[0-9]+$`；取 `sort -V` 的最后一个，
  patch + 1；若该 tag 已被占用就继续 +1，直到不冲突。没有基线时从 `0.0.0` 起，即 `v0.0.1`。
- 跳过：提交信息含 `[skip release]` 时整条流水线跳过。
- 流程：`release`（校验版本、建 tag、建 draft release）→ `verify` →
  `desktop`（Electron，诊断/回滚构建）与 `tauri`（Windows 发布线）+ `fnos` →
  `checksums` → `publish`。
- `publish` 在 `release`、`verify`、`tauri`、`checksums`、`fnos` 成功（或按既有规则
  skipped）时公开 draft。**M5 Task 3 起 `tauri` 是硬门**：Windows 腿失败时
  `needs.tauri.result == 'failure'`，`publish` 不跑；macOS/Linux 腿的失败被
  `continue-on-error` 在 `needs` 里归一为 `success`，不挡 Windows-only 发布（见 2.1）。
  `checksums` 继续证明安装包/签名/清单确实进了 Release。正式版用 `--latest`，预发布用
  `--prerelease`。
- **M5 Task 2 起**：自动发版发布的 Windows 安装包来自 **Tauri** 线；旧 Electron
  `desktop` job 仍会构建，但只产出 workflow artifact，且 `continue-on-error: true`。

### 4.3 手工触发（指定版本 / 预发布）

workflow 输入（引自 `release.yml` 的 `workflow_dispatch.inputs`）：

| 输入 | 用途 |
| --- | --- |
| `version` | 必填。正式版填 `3.14.0`；预发布填 `3.14.0` 且勾 `prerelease`，或直接填完整形式 `3.14.0-audit.20260922[.2]` |
| `prerelease` | `true` = 预发布（日期式版本）；`false` = 正式版（干净版本号，成为 GitHub Latest） |
| `build_artifacts` | 构建并上传桌面/CLI 产物；全部成功后才发布 |
| `keep_draft` | `true` = 即使成功也保持 draft |
| `notice` | 插入 Release 说明顶部的警示文本 |

正式版手工触发：

```bash
gh workflow run release.yml \
  -f version=<x.y.z> \
  -f prerelease=false \
  -f build_artifacts=true \
  -f keep_draft=false
```

规则：指定的正式版本**不能已存在**（tag 与 release 都会校验）；预发布版本不参与自动版本基线，
也不会成为 `/releases/latest/` 指向的对象。

### 4.4 发布后核对（M5 Task 2 之后）

```bash
gh release view <tag> --json isDraft,isLatest,assets \
  --jq '{draft:.isDraft,latest:.isLatest,assets:[.assets[].name]}'
```

目标状态至少包含：

- `Mode-<版本>-win-x64.exe`（Tauri 安装包，**本次 run 唯一上传的同名文件**）
- `latest.json`（Tauri 更新清单）
- `Mode-<版本>-win-x64.exe.sig`（安装包签名；`tauri build` 原名是
  `Mode_<版本>_x64-setup.exe.sig`，CI 改名后与安装包同名）
- `sha256.txt`（合并校验清单；`checksums` 由 Tauri Windows 腿的片段驱动）
- **不应出现**旧的 `latest*.yml` / `*.blockmap`：新发布线不上传它们
  （它们只属于旧 Electron release；与 `latest.json` 文件名不同，不会互覆）
- 旧 Electron 线的上一版资产仍在它自己的 release 里，没有被覆盖

期望：`draft=false`；正式版 `latest=true`。若任一资产缺失，发布不完整，按第 5 节止血，
不要用「重跑一次」掩盖缺失。

### 4.5 发布前检查（全部勾完才允许人推送 `main`）

- [ ] 3.1.1 的两个 CI secret 已创建（这是 `tauri` job 能产出 `.sig` 的前置条件；
      缺私钥时 `Build Tauri bundles` 会以非 0 退出，fail closed）。
- [ ] 2.1 的自动验证在本机与 CI 都是绿的（如实报告任何既有失败，不把失败写成通过）。
      **注意区分「已接线」与「已强制」**：重合度指标今天只接线、不阻断（M4 的
      `scripts/upstream-overlap-check.mjs` 尚未交付，缺失时步骤打印告警并 `exit 0`）；
      一旦该脚本出现，同一步骤自动改为阻断。不要把它读成「重合度已经验过」。
- [ ] 2.2 的 V1–V11 全部勾完；V11 若未完成，先完成 Task 1。
- [ ] 3.4 的签名检查全部勾完；`pubkey` 不是空串。
- [ ] 本次发布线是 **Tauri Windows-only**；旧 Electron 线的上一版资产仍可下载，
      回滚步骤（5.2 / 5.5）可用。
- [ ] 本次是 Windows-only，且已知「三平台 GA」是登记偏差（1.6）。
- [ ] 本地提交已报告给人；推送由人执行（4.1）。

---

## 5. 回滚

触发条件：Tauri 构建对用户是坏的——启动即失败/白屏、安装或覆盖重装失败、数据根被错误写入、
更新链把用户卡死，或 2.2 的人工项在发布后发现致命回归。

### 5.1 先止血

1. **停止继续推送 `main`**，避免自动发版在同一问题上叠新版本。
2. 记录现场：坏版本 tag、安装包名、失败项与原始输出；**不要删除坏 release 或其资产**（排障需要）。
3. 如果坏 release 还是 draft：不要公开；修好后重跑，或直接删除 draft 后重来。
4. 如果坏 release 已经公开且成为 Latest：执行 5.2。

### 5.2 主路径：重新发布上一个 Electron 版本

DR-M5-1 的前提：发布线切换是**单一提交**，且旧 Electron 线的构建能力在该提交里**不被删除**。
因此回滚不是「把用户降级」，而是**用旧线的构建能力发一个版本号更高的新 release**——
`electron-updater` 只按版本号向前更新，发一个更小的版本号用户收不到。

> **M5 Task 2 切换之后，这条路径仍然可执行。** 切换提交只把 `publish` 的产物来源换成
> Tauri，**没有删除** Electron 的 `desktop` job、`bundle:desktop` 脚本或旧 tag 的
> workflow。回滚机制就是下面第 4 步：**在上一 Electron tag 上重新触发 workflow**。
> 它用旧 tag 的提交和旧 tag 的 `release.yml` 构建 Electron、创建全新的 tag；旧 tag
> 的 Release 资产不会被本次 Tauri 发布触碰（不同 tag，且新流水线从不删除资产）。
>
> **切换后的更新通道**：新的 Tauri release 只带 `latest.json`，没有 `latest.yml`。
> 已安装的 Electron 用户在 `/releases/latest/download/latest.yml` 上得到 404，
> `electron-updater` 会保持当前版本（不会误把 Tauri 安装包当成 Electron 更新）。
> 只有从旧 tag 重新发布的回滚 release 才带 `latest.yml` 并成为 Latest，Electron
> 用户那时才会收到回滚版本。

步骤：

1. **找到上一个 Electron 版本 tag，并确认它的产物还在**

   ```bash
   gh release list --limit 20
   gh release view <上一个 Electron tag> --json tagName,isDraft,assets \
     --jq '{tag:.tagName,draft:.isDraft,assets:[.assets[].name]}'
   ```

   必须看到该 tag 的 `Mode-<旧版本>-win-x64.exe` 与 `latest*.yml`。
   Release 资产不会像 workflow artifact 那样 14 天过期；只要没人删 release，它们就还在。
   **不要对该 tag 执行 `gh release delete`，也不要在该 tag 上使用 `--clobber`。**

2. **确认旧 tag 的 workflow 具备本次需要的输入**

   ```bash
   git show <上一个 Electron tag>:.github/workflows/release.yml | sed -n '1,35p'
   ```

   需要看到 `workflow_dispatch` 的 `version`、`prerelease`、`build_artifacts`、`keep_draft`。
   若旧 tag 的 workflow 没有这些输入，改用 5.5 的 revert 路径。

3. **选一个新的、高于坏版本的正式版本号**

   例如坏版本是 `3.14.4`，回滚发布用 `3.14.5`。先确认不冲突：

   ```bash
   git ls-remote --tags origin "refs/tags/v<新版本>"
   gh release view "v<新版本>"   # 期望报 not found
   ```

4. **在旧 Electron tag 上手工触发 release**

   ```bash
   gh workflow run release.yml --ref <上一个 Electron tag> \
     -f version=<新版本> \
     -f prerelease=false \
     -f build_artifacts=true \
     -f keep_draft=false
   ```

   这里用的是 **`version` 输入**（必填）和 **`--ref <旧 tag>`**：workflow 会从旧 tag 的提交
   构建 Electron 线，并创建新 tag `<新版本>`；旧 tag 的资产不受影响。
   `prerelease=false` 让新 release 成为 `/releases/latest/`，旧线的 `electron-updater` 才会取到它。

5. **观察并核对**

   ```bash
   gh run list --workflow=release.yml --limit 5
   gh run watch <run-id>
   gh release view "v<新版本>" --json isDraft,isLatest,assets \
     --jq '{draft:.isDraft,latest:.isLatest,assets:[.assets[].name]}'
   ```

   期望：`draft=false`、`latest=true`，资产里有 `Mode-<新版本>-win-x64.exe`、`latest*.yml`、
   `sha256.txt`。然后在一台装有上一个 Electron 版本的机器上触发「检查更新」，确认能升到
   `<新版本>`。

### 5.3 已经在坏 Tauri 版本上的用户（M5 Task 1 演练后的定稿）

**结论：无自动降级路径。** 依据是 updater 的默认版本比较与一次真演练：

- Tauri updater 默认只接受**版本号更大**的清单；代码里没有开 `allowDowngrades`。
- 回滚 release 是 Electron 构建，**没有 `latest.json`**；Tauri 客户端访问
  `/releases/latest/download/latest.json` 得到 404，按 fail closed 处理，继续运行当前版本。
- 演练确认：签名/下载失败时不会安装，旧版本继续运行（不会「装一半」或静默降级）。

因此处置顺序是：

1. **首选：发一个版本号更高的 Tauri 修复版**（正向修复）。这是唯一能让已更新用户
   自动回到可用状态的路径。
2. **平台级止血**：按 5.2 从旧 Electron tag 发一个更高版本号的 Electron release，
   让**尚未更新**的用户和手动下载的用户拿到可用安装包。
3. **已装坏 Tauri 版的用户**：必须**手动**安装修复后的 Tauri 版或 Electron 安装包。
   不要向他们承诺「重发 Electron 就会自动回滚」——不会发生。
4. 如果坏版本就是 Tauri 发布线的第一个版本、且用户量还小：优先走 1；同时把坏 release
   标注「勿下载」但**保留**，供排障。

**不要做的**：不要为了「让用户降级」在清单里写一个更小的版本号（默认比较会直接忽略）；
不要删除坏 release；不要在旧 tag 上用 `--clobber`。

### 5.4 确保旧产物始终可下载

- 每次发布前（尤其是切换提交前）做一次冷备份到仓库外：

  ```bash
  mkdir -p "<仓库外的临时目录>/<旧版本>"
  gh release download <上一个 Electron tag> --dir "<仓库外的临时目录>/<旧版本>"
  ```

- 发布后再次核对旧 release 的资产列表（5.2 第 1 步），确认没有被本次上传覆盖。
- 不同 tag 的 Release 资产相互独立；**同名覆盖只会在对同一个 tag 使用 `--clobber` 时发生**。
  切换提交必须保证：新版本用新 tag，旧 tag 不动。
- workflow artifact 有 14 天保留期，**不能**把它当作回滚来源；回滚只依赖 Release 资产与旧 tag
  的构建能力。

### 5.5 备选：revert 切换提交（M5 Task 2 之后可用）

如果旧 tag 的 workflow 不可用，而切换确实是单一提交：

1. revert 该切换提交（不改其他提交）。
2. 人推 `main`；自动发版取「最近正式 tag 的 patch + 1」，`publish` 的产物来源
   随 revert 回到 Electron 线。
3. 按 5.2 第 5 步核对新 release 与更新。

这条路径依赖「旧 Electron 线的构建能力没有被切换提交删除」。**M5 Task 2 的切换提交
保留了 `desktop` job 与 `bundle:desktop` 脚本**，只把它的上传目标从 Release 改成
workflow artifact；revert 会把上传目标一并恢复。若将来有人删除 Electron 构建链，
本路径不可用，只能走 5.2。

### 5.6 回滚后验证与禁止项

- [ ] 新回滚 release 已公开且 `latest=true`。
- [ ] 旧 Electron 用户能从上一版升到回滚版本。
- [ ] 旧 release 的资产仍在，且没有被 `--clobber`/删除。
- [ ] 坏版本 release 保留供排障（必要时在说明里标注「勿下载」）。
- [ ] 已装坏 Tauri 版的用户得到了明确处置说明（正向修复或手动重装，见 5.3）。
- 禁止：用删除 release 当作回滚；用 `--clobber` 覆盖旧 tag；把「重跑流水线」当作修复；
  在未记录原始输出的情况下声称回滚完成。
