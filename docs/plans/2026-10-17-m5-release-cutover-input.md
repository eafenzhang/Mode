# M5 发布线切换的输入清单（M3 Task 5 的产物）

> 这份文档**不是 M5 的计划**，是 M3 交给 M5 的**输入**：产物长什么样、怎么装、切换发布线还差
> 哪几件事、以及哪几件事**只能人工跑**。来源与实测依据在
> `docs/specs/tauri-renderer-host.md` §13.4/§13.5/§13.6 与
> `.superpowers/sdd/2026-10-16-tauri-m3-ecosystem/task-5-report.md`（报告不入库）。
>
> 纪律提醒（沿用 M3）：**每一个「已验/未验」都要能指到命令与原始输出**；没有输出的结论写成
> 「未验」而不是「应该没问题」。

## 1. 产物名与安装方式

| 项 | 现状 | 依据 |
| --- | --- | --- |
| 打包器 | Tauri 2 的 NSIS bundler（`--bundles nsis`），Windows x64 | `.github/workflows/release.yml` 的 `tauri` job（matrix: `windows-latest` / `x86_64-pc-windows-msvc`） |
| 本机产物名 | `target/release/bundle/nsis/Mode_0.0.1_x64-setup.exe`（`0.0.1` 是 `tauri.conf.json` 的**占位版本**） | `tauri build` 的输出 |
| 发布契约名 | `Mode-<版本>-win-x64.exe`（CI 已按这个改名，`scripts/tauri-bundle-report.ts` 落 sha256 与体积） | `release.yml` 的 `Collect installer and record size` |
| 静默安装 | `Mode-<版本>-win-x64.exe /S /D=<目标目录>`（`/D` **必须放最后且不能加引号**） | 本机实测：`verify-installed.ts` 的 `installSilently` |
| 覆盖重装 | 同一个安装包再跑一次即可（NSIS 覆盖安装）；**已实测**，且重装后主 exe 与安装包内产物 sha256 一致 | §13.6 的 `install → reinstall → run` |
| 主 exe 名 | **`ModeTauri.exe`**（Cargo 的 `[[bin]] name`），**不是** `productName` | 本机实测踩到过：写死 `Mode.exe` 会「装完了却报找不到 exe」 |
| 安装目录内容 | `ModeTauri.exe` + `WebView2Loader.dll` + `uninstall.exe` + `host-sidecar/`（bundle 与 externals）+ `node/node.exe` + `config/provider/mode-builtin.json` | §13.6 的清单 |

**M5 需要决定的第一件事**：`ModeTauri.exe` 与旧 Electron 线的 `Mode.exe` **名字不同**。发布契约
只管安装包名，但用户界面上（快捷方式、任务管理器、`mode://` 的注册命令）会看到这个差异；
要么在 M5 把 bin 名改成 `Mode`，要么接受两条线在过渡期名字不同。这条**不影响安装包能否发布**，
但它属于「切换发布线」的一部分，列在这里免得漏。

## 2. 体积门禁：实测值与**待定**的数值

实测在**门禁脸**（NSIS 安装包字节）上，完整口径与原始输出见
`docs/specs/tauri-renderer-host.md` §13.2/§13.4。摘要：

| 预算 | 实测（门禁脸） | 现门禁 | 状态 |
| --- | --- | --- | --- |
| **(a) 核心**（壳 exe + UI 资产 + Host bundle + `config/`） | **23.29 MiB** | **≤30MB（沿用原值）** | **达标** |
| 总包（产品档，含随包 Node + externals） | **51.5x MiB** | — | 报告值 |
| **(b) Node 运行时 + sidecar externals**（= 总包 − 核心，整包 LZMA 的增量） | **~28.2 MiB** | **数值待人定** | DR-T5-3：Task 5 只给数字与建议 |

**建议（供人决定，Task 5 未自行改门禁）**：

- (a) **不需要改**：≤30MB 在正确的脸上是达标的。此前认为它不可达，是因为那个 54.41 MiB 量的是
  **未压缩文件字节和**（且不含壳 exe）——换到安装包字节面之后它压到了 23.29 MiB。
  Ruling 31 的放宽是**人决定的既成事实**，本清单不翻案，只把「当初的理由与实测不符」记下来。
- (b) 建议给一个**有头寸**的预算，例如 **≤35 MiB**（当前 28.2 MiB，头寸约 24%），或者直接
  按「总包 ≤60 MiB」管。**数值由人拍**。
- **唯一能兑现原 ≤30MB 总包的替代路径**（不是本阶段的建议，只是让取舍可见）：把 Host/services
  层用 Rust 重写——即 DR-M3-1 拒绝的那条慢路。

## 3. 更新链需要什么（**全部未做，属 M5**）

现状（`crates/updater/src/lib.rs` 的文件头与它的 conf↔常量守卫）：

- **端点已声明**：`https://github.com/eafenzhang/Mode/releases/latest/download/latest.json`
  （与旧 Electron 线**同一个 release feed**，`latest.yml` 与 `latest.json` 是两个不冲突的资产）。
- **`pubkey` 是空串**：`tauri-plugin-updater` 的 `Config.pubkey` 是**必填字段**（缺席会让 app
  启动即 panic，实测过），空串在 `verify_signature` 里会解码失败 → **fail closed**，不会静默放行
  未签名的更新。M5 换成真密钥即可。
- **Release 里目前没有 `latest.json` 这个资产**：CI 只上传 Electron 线的 `latest*.yml`。

M5 要做的三件事：

1. **签名密钥**：生成 Tauri updater 的密钥对，私钥进 CI secret
   （`TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`），公钥写进
   `tauri.conf.json` 的 `plugins.updater.pubkey`。**私钥一旦轮换，已发布的旧版本就无法再信任新
   版本**，所以密钥要在第一次对用户发布**之前**定下来。
2. **`latest.json`**：`tauri build` 在设置了签名密钥时会产出；要把它作为 release 资产上传。
   内容里的 `pub_date`/`version` 必须与 tag 同源（版本号已经在 CI 用
   `--config '{"version":"…"}'` 注入，`tauri.conf.json` 里的 `0.0.1` 只是占位）。
3. **三平台演练**：M3 只出并验了 Windows（Ruling 20）。切换发布线需要 macOS/Linux 产物；这两条
   腿在 CI 里仍是 `continue-on-error`，且 §8.3 的两条已知风险（Linux 的 webkit2gtk/libsoup3
   依赖名、macOS 的 aarch64 目标确认）**未解决**。解冻条件就是那两条的收口。
   Windows 上得到的体积/Node 运行时结论对三平台**有可迁移性，但不构成验证**。

另外两件与发布线切换直接相关的事：

- **旧线的 `--clobber` 风险**：Tauri 线的产物名与 Electron 线**逐字相同**，所以 CI 现在把
  Tauri 产物**只上传为 workflow artifacts**。切到 Release 时要一次切干净，否则同名上传会把用户
  正在下载的 Electron 安装包覆盖掉。
- **代码签名证书**（与 updater 密钥是两回事）：NSIS 安装包如果不签名，Windows SmartScreen 会报
  未知发布者。属于发布工程，M5 决定。

## 4. 发版前**必须人工跑**的事（DR-T1-2 划出的那批 + Task 5 追加）

这些**不能进 CI**（要真 webview / 真安装器），漏掉它们等于「路径静默坏掉而发布流程说全绿」——
本仓库反复吃亏的正是这一类。

| # | 命令 | 判据 | 出处 |
| --- | --- | --- | --- |
| 1 | `MODE_TAURI_WEBVIEW_ROUNDTRIP=1 node --import tsx --test apps/desktop-tauri/test/webviewRoundTrip.test.mts` | 成功路径 `status: "ok"` / `gateOpened: true` / `portIsRealMessagePort: true`；失败路径 `failedPhase: "preparing_host_storage"` + `errorCode: "sql_failed"` | Host spec §5、§10.8 |
| 2 | `MODE_TAURI_PRODUCT_UI=1 node --import tsx --test apps/desktop-tauri/test/productUiMount.test.mts` | `mounted === true`、`acceptedFrames > 0`、`pumpFramesGrewAfterReady === true`、`uiText` 不含 `not_implemented_until_m3`；负向对照 `mounted === false` | Host spec §5、§10.9/§10.11 |
| 3 | `node --import tsx apps/desktop-tauri/scripts/verify-installed.ts --installer <产品档安装包>` | 22 条断言全过（含**覆盖重装**、内嵌产品页、随包 Node + 资源目录档、**真开一个终端**、数据隔离、无孤儿） | Host spec §13.6 |
| 4 | `node --import tsx apps/desktop-tauri/scripts/measure-cold-start.ts --release <安装后的 exe>` | 7/7 有效样本；中位数在 §13.5 的实测值量级（release 约 **825 ms / 962 ms**，首样本约 1.1 s），**不出现 `startup_failed`** | Host spec §13.3/§13.5 |
| 5 | `node scripts/rust-cargo.ts clippy --features $FEATURES --bins -p mode-tauri` | 三个 `required-features` 门住的 opt-in harness 单独 lint 干净（`--all-targets` 覆盖不到它们） | Host spec §20 第 4 条 |

第 3–4 条是 Task 5 追加的：**它们覆盖的正是「非打包壳 ≠ 安装后的产物」这一段**，而本轮之前
这一段从未被走过（那场 `\\?\` 入口崩溃只在非打包 release 壳上验证过）。

## 5. CI 侧的接线（切换时要改的地方）

- `tauri` job 的产物上传目标：workflow artifacts → Release 资产（**同时**考虑旧线的同名冲突）。
- 该 job 目前**不在 `publish` 的 `needs` 里**，且 mac/linux 两条腿 `continue-on-error`
  （`docs/specs/tauri-platform-commands.md` §8.1）；转正时两者一起去掉。
- `beforeBuildCommand` 已经是 `build:package-inputs`（= 产品页 + sidecar 发运集），
  所以 CI 出的包**已经**含 sidecar 与随包 Node，不需要额外步骤。
- 体积留痕：`scripts/tauri-bundle-report.ts` 记的是**同一个门禁脸**（安装包字节），无需改动；
  两份预算的差值只在需要时用 `measure-installer-budgets.ts` 复算（它要出三个包，不进 CI）。

## 6. 已知边界与未验证（**不要读成「没问题」**）

1. **macOS / Linux 产物与体积完全未验**（Ruling 20 的范围收窄，解冻条件见 §3.3）。
2. **更新链端到端未演练**：`latest.json` 没发布过，签名密钥没生成，升级路径没跑过。
3. **`windows_subsystem = "windows"` 的发布构建里，平台面清单没有「用户可见」的落点**：
   只有把 stderr 接成管道的发起进程才能拿到（§13.6 已实测这条是通的）；从资源管理器双击启动时
   它没有落点。§20 第 1 条仍是开放项，代价与两个候选设计写在报告里。
4. **验收台（bench）不进发布产物**，但它的开发档命令仍在（`build:frontend-with-bench`）——
   切发布线时不要顺手把它打进发布档（它多 1.73 MiB 且暴露内部命令面）。
5. **崩溃重启策略仍未做**（Host spec §14，DR-T2-4）：就绪后崩只推 `failed(transport_closed)`，
   不重启。用户可见行为是失败屏。
6. **`disk` 摘要恒为 `[]`、`sequence` 归属未定**：两条都在 §20 保持可见，条件未触发。
