# M5 发布与收口 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `subagent-driven-development`（推荐）或
> `executing-plans` 逐任务实现。步骤用 `- [ ]` 复选框跟踪。
>
> **开工前必读**：M3 的交接文档 `docs/plans/2026-10-17-m5-release-cutover-input.md`（产物名、安装方式、
> 更新链所需、人工前置项——**本计划的输入**）、路线图 M5 节、
> `docs/specs/tauri-platform-commands.md` §8（产物去向与体积口径）、
> `docs/specs/tauri-renderer-host.md` §13.4 与 §20（数字矛盾与开放项）。

**Goal:** 让 **Tauri 版成为发布线**：`release.yml` 产出并发布 Tauri 产物，更新链真正可用，且
「发布」这件事有一条可执行、可回滚、含人工前置项的清单。

**Architecture:** 不改产品逻辑。M5 是**发布工程**：把已经验过的 Windows 产物接进 release 流水线，
补上更新链（签名密钥 + `latest.json` + 真更新演练），并把旧 Electron 线从发布路径上退下来——
但要**保留回滚能力**。

**Tech Stack:** GitHub Actions（`release.yml`）、tauri bundler、tauri updater（`latest.json` + minisign）。

**Spec:** 更新 `docs/specs/tauri-platform-commands.md` §8（产物去向与体积基线）、
`docs/specs/tauri-renderer-host.md`（§13.4 的数字矛盾、§20 开放项），并产出 `RELEASE.md`
（或等价的发布清单）。

## Global Constraints

- **Windows only，除非人解冻**（Ruling 20）。这条会与路线图 M5 验收的「三平台 GA」冲突——
  见 DR-M5-2，**必须由人决定**，不许静默收窄。
- **不改产品逻辑**：M5 只动发布工程与文档；需要改产品才能发布时**停下来回报**。
- **发布不可由 agent 执行**：按 `AGENTS.md`，agent 只做本地提交，`git push` 由人执行（push `main`
  会自动发版）。所以本计划的每个任务都止于「可推送、已本地验证」，**推送与发版由人按**。
- **旧线必须可回滚**：切换发布线时，旧 Electron 产物与最近一个可用版本不得被抹掉。
- **体积口径按整包（NSIS 安装包字节）**：M3 实测整包 **51.50 MiB**、核心 23.29 MiB；
  原门禁「≤30MB」指的是**整包**。数字矛盾已登记（spec §13.4），**本计划不自行改数**。

---

## 决策记录（开工前先读——三条待人拍板）

### DR-M5-1：发布线切换需要一个**原子性**的做法，否则两边同名互相覆盖

Tauri 产物与旧 Electron 线的文件名逐字相同（`Mode-<版本>-win-x64.exe`）。M1–M4 期间 Tauri 只发
workflow artifacts，正是为避免同名 `--clobber` 掉用户正在下载的安装包。M5 切换时，**同一次
workflow run 里不能既上传 Electron 又上传 Tauri 到同一个 release**。

**决策**：切换以一个明确的提交完成——`publish` 的产物来源从 Electron 换成 Tauri，**同时**把旧线
产物保留为上一个 release 的资产（不删除、不覆盖）。**代价**：用户在切换点看到的是「新版本换了
实现」，而文件名与安装方式没变；若 Tauri 版有问题，回滚方式是**重新发布上一个 Electron 版本**
（所以旧线的构建能力不能在同一提交里被删）。

### DR-M5-2：**已定：A（Windows-only 先行）**——「三平台 GA」登记为偏差

路线图 M5 验收写的是「**三平台** GA 发布」。而 Ruling 20（人）把 macOS/Linux 移出了 M3，
两条腿从未出过真产物、也从未验证。

**决定（人，2026-10-17）**：走 **A——只发布 Windows 的 Tauri 版**，并把「三平台 GA」这条验收
**登记为偏差**（同 Ruling 20/26 的做法：不改路线图原文，写清原因、代价与解冻条件）。

**代价**：Windows 之外的用户在切换后**没有 Tauri 版可用**；若旧 Electron 线在某平台仍可下载，
回滚/并行策略需在 `RELEASE.md` 里写明（见 DR-M5-1）。**解冻条件**：需要 macOS/Linux 产物时，
按 M1 已登记的两条已知风险（Linux 的 webkit2gtk/soup3 包名与 AppImage 的 FUSE 绕过；macOS 需确认
出的是 arm64）重新开一次决策，并把本条偏差撤销。

### DR-M5-2b：**已定：体积门禁放开**——改为持续测量项

**决定（人，2026-10-17）**：「体积门禁放开」= 体积**不再作为发布阻断**。

口径不变的部分：仍按**整包（NSIS 安装包字节）**测量并登记（M3 实测整包 51.50 MiB、
核心 23.29 MiB），每个数字必须标注它测在哪张脸上（DR-T5-1），剪枝仍同时报原始/压缩收益（DR-T5-2）。
**变的是阈值**：不设阻断值，也不自行编一个新的数字。**重新开启条件**：弱网安装或分发渠道对体积
提出硬限制时，重开本条——届时最便宜的杠杆不是重写后端，而是别把插件/技能负载打进包。
**代价**：安装包会明显大于原目标的 ≤30MB（原目标指整包）。

### DR-M5-3：更新链的密钥与清单是发布工程的核心，不是可选项

Tauri 的 `Config.pubkey` 是**必填字段**，M1–M3 期间一直留空串——语义是**fail closed**
（`PublicKey::decode("")` 报错），即「装了也无法更新」。M5 必须：生成密钥、把它作为 CI secret、
用 CLI 对产物签名、发布 `latest.json`，并**在 Windows 上真跑一次更新演练**（旧版 → 新版）。

**代价（若判错）**：私钥泄露等于可向用户推任意更新；所以私钥只进 CI secret、不落仓库，
并写明轮换方式。

---

### Task 0: 发布清单与回滚口径（先写清楚，再动流水线）

**Files:** Create `RELEASE.md`（发布清单：构建、验证、签名、发布、回滚、人工前置项）；
Modify `docs/specs/*` 的相关节（如需要）。

- [ ] **Step 1**: 把 M3 交接文档的输入整理成 `RELEASE.md` 的五段：**构建**（命令与产物名）、
  **自动与人工验证的分界**（opt-in webview 测试、真鼠标拖拽与视觉、更新演练——这些**不在 CI**，
  必须是发版前清单项）、**签名与清单**、**发布**、**回滚**（含「重新发布上一版 Electron」的具体步骤）。
- [ ] **Step 2**: 在 `RELEASE.md` 里明确写死：**发版由人推送触发**，agent 只到「本地可推送」为止。
- [ ] **Step 3**: 提交。

### Task 1: 签名密钥与 `latest.json`

- [x] **Step 1–5（大纲，开工时出子计划）**：生成密钥（私钥进 CI secret，**不入库**）、
  `tauri build` 携带签名、发布 `latest.json`、把 pubkey 从空串换成真值（**这会让 M1 的那条
  「空串 fail closed」变成「真密钥校验」**，须同步改 spec 与守卫）、Windows 上旧版→新版真更新演练。
  完成记录：`RELEASE.md` 的 1.4 / 2.2 V11 / 3.1–3.4 / 5.3；`createUpdaterArtifacts: true`
  只产出 `.sig`，`latest.json` 由 `scripts/tauri-update-manifest.ts` 生成。

### Task 2: 切换发布线（DR-M5-1）

- [x] **Step 1–5（大纲）**：`publish` 的产物来源从 Electron 换成 Tauri；旧线产物作为上一 release 的
  资产**保留**；验证「同一次 run 不会既传 Electron 又传 Tauri 到同一 release」；本地 dry-run
  （不 push）；提交。完成记录：`.superpowers/sdd/2026-10-16-tauri-m3-ecosystem/task-m5-2-report.md`。

### Task 3: 三指标门禁与收口

- [ ] **Step 1–4（大纲）**：把 M5 验收里的指标接进必跑门禁（具体是哪三项以路线图 M5 节为准）；
  处理 spec §13.4 的**数字矛盾**（待人给口径）；把 §20 的开放项逐条关闭或登记；提交。

---

## 阶段验收

1. `RELEASE.md` 就位，含构建/验证分界/签名/发布/**回滚**五段，且明确「推送由人执行」；
2. 更新链在 Windows 上**真跑通过**（旧版 → 新版），pubkey 不再是空串，spec 与守卫同步；
3. 发布线切换后：Tauri 产物进 Release，**旧线产物保留可回滚**，无同名互覆；
4. 平台范围按 DR-M5-2 的决定处置——若选 A，则「三平台 GA」作为**登记偏差**写明；
5. spec §13.4 的数字矛盾有人给口径（不自行改）；
6. 人工前置项（opt-in webview 测试、真拖拽与视觉、更新演练）写进 `RELEASE.md` 且**逐项可勾**。

## 风险

| 风险 | 对冲 |
| --- | --- |
| 同名产物互覆，用户在切换点下到坏包 | DR-M5-1：切换以单一提交完成，旧产物保留；本地 dry-run |
| 私钥入库或泄露 | 只进 CI secret，不入仓库；写明轮换方式 |
| 「三平台 GA」被静默收窄 | DR-M5-2 摆明两条路，未定前不写死；选了就登记偏差 |
| 只有 CI 绿而人工项没做 | `RELEASE.md` 把人工项列成可勾清单，且写明它们不在 CI |
| 又拿错尺子谈体积 | 沿用整包口径（NSIS 安装包字节）；§13.4 的矛盾交人 |
| 回滚没有真实路径 | 回滚步骤必须是「重新发布上一个 Electron 版本」的具体命令，不是「回滚即可」 |
