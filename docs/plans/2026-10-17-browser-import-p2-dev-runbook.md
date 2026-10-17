# M0 browser-import P2 真机验收 runbook（dev 模式，路线 A）

关联：`docs/plans/2026-10-15-browser-import-helper-rust-port.md`（Task 7）、
`docs/plans/2026-10-15-browser-import-abe-spike.md`（A/B/C 决策）、
`docs/specs/browser-import-helper-contract.md`。

**目的**：在真实 Windows 会话里抓一次参照——允许 UAC 与拒绝 UAC 两条路径的宿主行为、
helper 的 reason 与耗时。**不要求**这一轮拿到成功密钥（spike 已证明当前 Chrome 上旧实现
也可能止于 `broker_initialization_failed`）。

## 本轮已准备好的东西

- **dev-only 激活入口**：`BrowserSettingsSection.tsx` 在 `import.meta.env.DEV` 为真时显示
  Windows 的「导入浏览器数据」按钮；生产构建仍然隐藏。
- **dev-only 品牌兼容**：`windowsChromeAppBoundKey.ts` 在
  `MODE_BROWSER_IMPORT_ALLOW_LEGACY_ORACLE=1` **且未打包**时接受 C# oracle 的
  `ZCODE_BROWSER_IMPORT_HELPER` / `ZCODE_BROWSER_IMPORT_V1`。生产路径不受影响。
- **C# oracle 已放到 dev 解析路径**：
  `packages/desktop/bundled-tools/win32-x64/browser-import/mode-browser-import-helper.exe`
  （当前 `--version` 输出 `ZCODE_BROWSER_IMPORT_HELPER …`）。
- Rust P1 产物备份在 `.plan/rust-helper-backup.exe`。

## 你需要准备

1. Windows x64 测试机，Chrome 已安装。
2. 用要导入的 profile **登录至少一个网站**（让 `Cookies` 库里有真实条目）。
3. 记录：Chrome 版本（`chrome://version`）与 Profile Path。**不要**提供 cookie 内容或密钥。

## 运行步骤

在仓库根目录用 Git Bash：

```bash
MODE_BROWSER_IMPORT_ALLOW_LEGACY_ORACLE=1 pnpm dev:desktop
```

PowerShell：

```powershell
$env:MODE_BROWSER_IMPORT_ALLOW_LEGACY_ORACLE="1"
pnpm dev:desktop
```

1. 打开 **设置 → 浏览器**，确认出现「导入浏览器数据」按钮（dev 构建才显示）。
2. **Run A（允许 UAC）**：点击按钮 → UAC 弹窗点「是」。
   - 记录 UI 上的导入结果（成功条数 / 错误码）与终端里 `[browser-data]` 开头的日志。
   - 预期可能是 `broker_initialization_failed`（spike 已见）或成功；**两种都算有效参照**。
3. **Run B（拒绝 UAC）**：再点一次 → UAC 弹窗点「否」。
   - 预期错误码：`chrome_cookie_elevation_cancelled`。
   - 若没有弹 UAC，停在这里，把终端日志与 UI 错误码发回——说明提权前的路径有问题。

## 需要回传的内容

- Chrome 版本 + Profile Path（不要 cookie/密钥）。
- Run A / Run B 的 UI 结果与错误码。
- 终端里包含以下关键词的行（可直接复制）：
  - `[browser-data]`
  - `helper 响应（legacy oracle，dev）`
  - 任何 `chrome_cookie_*` 错误码。
- 是否真的出现了 UAC 弹窗（是/否），以及弹窗标题。

## 验收后恢复

```bash
pnpm --filter @mode/desktop prepare:browser-import-helper   # 重建 Rust P1，覆盖 C# oracle
unset MODE_BROWSER_IMPORT_ALLOW_LEGACY_ORACLE               # 或关闭终端
```

- dev 的品牌兼容与按钮显示都只在 `import.meta.env.DEV` / 环境变量下生效，发布构建不受影响。
- **打包态**验收不在这份 runbook 里：它要求 app 与 helper 都有有效 Authenticode 且同证书；
  当前仓库没有签名配置，需要单独决定是否接入代码签名。

## 已知边界

- C# oracle 是**参照物**，不是要发布的实现；它的 `ZCODE_*` 品牌正是 Rust 版要修的缺陷。
- 成功路径没有参照：若 Run A 仍回 `broker_initialization_failed`，按 spike 的 A/B/C 决策处理——
  A 得到参照后再判断是否需要走 B（浏览器进程内调用 + 安全评审）。
