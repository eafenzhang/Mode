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

---

## 2026-10-17 执行记录（agent 代跑）

**环境**：Windows x64；管理员会话（`IsInRole(Administrator)=true`）；
`EnableLUA=1` / `ConsentPromptBehaviorAdmin=0`（管理员静默提权，**不弹 UAC**）；
Chrome `155.0.8059.84`；profile `Default`（33 条 cookie）。

| 步骤 | 结果 |
| --- | --- |
| 不带 `allowElevatedChromeDecryption` | `success:true`；cookies `0/33/0`；issues `["chrome_cookie_elevation_required"]`（预期：未授权提权就不尝试 v20） |
| 带 `allowElevatedChromeDecryption:true` | helper 经 legacy 品牌兼容被调用；原始响应 `ZCODE_BROWSER_IMPORT_V1\tERR\tdecryption_failed`（exit 0）；宿主报 `chrome_cookie_app_bound_decryption_failed`；cookies 仍 `0/33/0` |
| 直接黑盒跑 helper（同一请求） | stdout 同上；stderr 空；exit 0 |

**UAC 取消路径未验**：本机是管理员会话且 `ConsentPromptBehaviorAdmin=0`，提权静默、没有弹窗可点
「否」。要验取消路径需要**非提权会话**或临时改 UAC 策略（后者不应在未授权时做）。

**参照结论（路线 A）**：旧 C# 实现在宿主驱动的真实链路上止于 `decryption_failed`
（不同于 spike 直跑时的 `broker_initialization_failed`）——broker 已初始化，失败在解密/电梯调用阶段。
**仍无成功路径**；接下来是选 B（进程内调用 + 安全评审）还是 C（C# 保持原位、Rust 停在 P1）。

**恢复**：Rust helper 已重建并自检为 `MODE_BROWSER_IMPORT_HELPER 2 x64 0.0.1 257739bc`；
dev app 已停止；未修改系统 UAC 策略。

> **2026-10-18 追加（重要 confounder）**：上面所有结果都产生在**管理员会话**中。
> 后续用受支持的 Chrome 扩展路径探测发现：同一真实 profile（33 条 v20）在管理员会话下
> 扩展读到 0 条，而 cookie 库未被删除。**非提权会话复测前，不应把 `decryption_failed`
> 当作“app-bound 不可行”的结论**。详见 B spike 的 T1 执行记录。
