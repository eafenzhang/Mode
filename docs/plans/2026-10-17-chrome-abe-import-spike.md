# Chrome App-Bound 导入：独立 spike（B 路线）

> **决策**：2026-10-17 用户选 **B**——若产品必须支持当前 Chrome 的 v20 cookie 导入，
> 把「浏览器进程内调用 / 注入 + 通道」立为**独立 spike**，含安全评审，不混进 M0 收尾。
>
> 本文件只**立项与划边界**，不含生产实现。关联：
> `docs/plans/2026-10-15-browser-import-abe-spike.md`（事实与 A 参照）、
> `docs/specs/chrome-abe-import-security.md`（安全要求）、
> `docs/specs/browser-import-helper-contract.md`（现有 helper 契约）。

## 已确认的事实（不再重复验证）

1. Chrome 的 app-bound 密钥由 `IElevator` / `IElevator2` 的 `DecryptData` 解封；调用方需要
   浏览器进程身份才能通过路径校验（公开 Chromium 文档/源码与 xaitax PoC README）。
2. 本机 Chrome `155.0.8059.84`；`Local State` 有 `os_crypt.app_bound_encrypted_key`（`APPB` 前缀）。
3. **A 参照（2026-10-17，宿主链路）**：C# oracle 经宿主调用返回
   `ERR decryption_failed`（exit 0，stderr 空）；宿主映射为
   `chrome_cookie_app_bound_decryption_failed`；cookies `0 imported / 33 skipped`。
   直跑旧产物此前得到 `broker_initialization_failed`——说明 **broker 初始化能过，
   失败点在解密/电梯调用本身**；成功路径仍无参照物。
4. 本机是管理员会话且 `ConsentPromptBehaviorAdmin=0`，提权静默、没有 UAC 弹窗；
   取消路径尚未验收（需要非提权会话或经授权的 UAC 策略变更）。

## 目标（可判定的问题）

**在不降低安全边界、不绕过授权、不外传数据的前提下，能否支持当前 Chrome 的 v20 cookie 导入？**

产出：一条被安全评审背书的**选定路径**，或一个明确的 **no-go**；若 go，另出实现 spec + 计划。

## 非目标

- 不在 spike 阶段把注入/进程内调用合入 `main`。
- 不绕过 Chrome 的安全机制、企业策略或用户授权；不访问非当前用户的 profile。
- 不记录、外传或持久化 cookie 值与解封后的密钥。
- 不阅读上游 `zai-org/ZCode` 或旧 C# 实现体（clean-room）；只读公开文档与黑盒行为。
- 不改变 M0 的收尾口径；C# 在结论出来前保持原位。

## 安全与合规前置（任何 PoC 开工前）

- 只处理**本机当前用户自己的** Chrome profile；由用户的明确动作触发，不做后台/常驻。
- PoC 在独立分支/独立目录，**不得进 main**；评审通过前不发布。
- 最小权限；能不用管理员就不用；不新增 Windows 服务（除非评审明确接受）。
- 代码签名、完整性校验、可审计（只记数量与 reason，不记值）。
- 评估 AV/EDR 误报、企业策略、Chrome 更新兼容与用户信任成本。
- 法务/ToS 评估：用户自有数据、Chrome Web Store 政策、企业与学校设备限制。

## 候选路径（按风险从低到高）

### P1：Chrome 扩展 + Native Messaging（**优先评估**）

- 用 MV3 扩展的 `cookies` 权限读取用户已登录站点的 cookie；经 Native Messaging 交给本应用。
- 优点：**受支持的 API**；无注入；Chrome 自己解密 v20；企业可用策略放行。
- 代价：需要用户安装扩展（商店或侧载）；扩展权限提示；Native Messaging host 的安装清单；
  商店政策与维护成本。
- 待回答：`chrome.cookies.getAll` 是否覆盖 HttpOnly / 目标域；扩展权限范围能否最小化；
  商店/企业策略是否允许「导入到本应用」这一用途。

### P2：CDP（`--remote-debugging-port` + `Network.getAllCookies`）

- 受支持但需要用户以调试参数启动 Chrome；新版 Chrome 对默认 profile 的远程调试有额外限制。
- 安全风险高（调试端口是本机任意进程可用的强大接口），不适合作为产品默认路径。
- 只作为诊断/对照，不作为发布方案。

### P3：COM elevator 的受支持激活

- 继续调查 `IElevator2` 的激活模型（服务激活 / 浏览器进程身份要求 / 路径校验）。
- 若存在**受支持的**调用方式（无需注入），优先于 P4；否则明确记录为不可行。
- 只用黑盒探针与公开文档；不读 Chrome 实现。

### P4：浏览器进程内调用 / 注入（最后手段）

- 在 Chrome 浏览器进程内调用 `IElevator2::DecryptData`，经最小 IPC 通道回传密钥/数据。
- 风险：注入框架、AV/EDR 误报、Chrome 更新破坏、权限与信任成本、潜在法律风险。
- **只有 P1–P3 全部被否证后**才进入；必须先过安全评审，再做最小 PoC。

## 工作分解

| 任务 | 内容 | 产出 | 预估 |
| --- | --- | --- | --- |
| T0 证据板 | 复现 A 参照；记录 Chrome 版本矩阵、电梯注册、公开文档出处 | 事实清单（本文件续写） | 1–2 天 |
| T1 扩展 PoC | MV3 最小扩展 + Native Messaging host，读当前 profile 的 cookie 并回传计数/样本（不落盘值） | PoC 分支 + 可行性报告 | 3–5 天 |
| T2 CDP 评估 | 验证新版 Chrome 的限制与安全代价 | 结论段落 | 1 天 |
| T3 电梯激活调查 | 不同调用身份/父进程/路径的黑盒探针 | 可行/不可行结论 | 2–3 天 |
| T4 注入 PoC（条件触发） | 仅当 T1–T3 全否；最小、无外传、可关闭 | PoC + 安全评审材料 | 1–2 周 + 评审 |
| T5 决策 | 选定路径或 no-go；若 go，出实现 spec 与计划 | 决策记录 | 1 天 |

## 退出标准

1. 一份书面结论：选定路径 + 安全评审签字，或明确 no-go；
2. 若 go：新 spec（威胁模型、权限、数据流、失败语义、回滚）与实现计划；
3. 若 no-go：C#/P1 保持现状，M0 的 C# 归零继续挂起，并在 roadmap/port plan 登记。

## 风险与对冲

| 风险 | 对冲 |
| --- | --- |
| 注入被 AV/EDR 判为恶意 | 优先 P1–P3；P4 必须签名、最小化、可关闭，并做多引擎测试 |
| Chrome 更新使路径失效 | 版本矩阵 + 黑盒回归；把版本检测与失败语义写进 spec |
| 企业策略/商店政策禁止 | P1 先做政策评估；必要时只支持侧载 + 管理员放行 |
| 用户信任与隐私 | 明确 per-import 授权、不落盘密钥、只记计数、可审计 |
| 范围蔓延进 M0 | 本 spike 独立分支/独立评审；M0 只保留“挂起”状态 |
| clean-room 违规 | 只读公开文档与黑盒行为；不打开旧 C# 实现体 |

## 关联变更

- `docs/plans/2026-10-15-browser-import-abe-spike.md`：追加 B 决策与本文件链接。
- `docs/plans/2026-10-15-browser-import-helper-rust-port.md`：P2 的 Task 4/5 标记为
  **移交本 spike**；C# 保留；P3 删除 C# 继续挂起。
- `docs/plans/2026-10-09-tauri-rewrite-roadmap.md`：M0 任务 6 追加 B 的登记。
