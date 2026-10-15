# Windows 浏览器数据导入 · App-Bound 解封 spike（2026-10-15）

关联：`docs/specs/browser-import-helper-contract.md`、`docs/plans/2026-10-15-browser-import-helper-rust-port.md`（P2）。
状态：机制面已探明一半，**卡点已定位**；成功路径缺参照物。

## 已确认的事实

1. **机制（公开资料）**：Chrome 的 app-bound 密钥不是用 DPAPI 直接解的，而是交给它自己的
   **COM elevator 服务**解封；接口为 `IElevator` / `IElevator2`（Chrome 144+ 起为 `IElevator2`），
   关键方法 `DecryptData`。该服务的调用方需具备浏览器进程身份以通过路径校验。
   来源：公开分析（xaitax 的 App-Bound Encryption PoC README）+ 公开 Chromium 源码。

2. **接口 IID（公开 Chromium 源码 `chrome/elevation_service/elevation_service_idl.idl`）**：
   `IElevator` = `A949CB4E-C4F9-44C4-B213-6BF8AA9AC69C`；
   `IElevator2` = `8F7B6792-784D-4047-845D-1782EFBEF205`；
   `IElevator2Chrome` = `1BF5208B-295F-4992-B5F4-3A9BB6494838`（另有 Chromium/Beta/Dev/Canary 变体）。

3. **本机环境已具备部分条件**：Chrome `155.0.8059.80` 已安装并在运行；
   `GoogleChromeElevationService` 已注册（STOPPED，按需启动）；
   `Local State` 内 `os_crypt.app_bound_encrypted_key` 存在（`APPB` 前缀，860 个 base64 字符）；
   `HKLM\SOFTWARE\Classes\Interface\{1BF5208B-…}` 有 `TypeLib` + `ProxyStubClsid32` 登记，
   与 Chrome 155 的接口 IID 一致。

4. **关键否证（改变了 P2 的可行性判断）**：用**真实**密钥 + 真实 `chrome.exe` 路径驱动
   **旧 C# 产物**，得到 `MODE_BROWSER_IMPORT_V1 → ERR broker_initialization_failed`
   （exit 0；Chrome 已在跑的对照下同样失败）。
   → 旧实现在这台机器上**也拿不到成功路径**，因此**不能拿它当成功路径的参照物**；
   同时也说明它对新版 Chrome 可能已不适用。

5. **CLSID 未定位**：`HKLM\SOFTWARE\Classes\CLSID\{708860E0-F641-4611-8895-7D867DD3675B}`
   键存在但**无** `InprocServer32` / `LocalServer32` / `AppID`；`HKCR\CLSID` 全量按数据搜
   `elevation_service.exe` **无命中**；`HKCU\SOFTWARE\Classes\CLSID` 下无同名键。
   → **激活模型未明**（可能经服务激活，或仅对浏览器进程开放）。

## 结论

- P2 **不是「调一个 COM 方法」**：elevator 的路径校验面向浏览器进程。落地只有两条路——
  在浏览器进程内完成调用（即注入），或存在我们尚未找到的受支持通道。两条都需要独立 spike。
- 成功路径没有参照物，所以验收口径要换成**更强的自证**：用实现取回 32 字节密钥后，
  **拿它去解开 Chrome `Cookies` 库里的真实密文（AES-256-GCM）**；解得开即证明密钥正确。
  这比与原实现对拍更强，也不依赖旧产物能否跑通。
- **安全敏感性**：若结论落到进程注入，该实现属安全敏感代码，需要专门评审，
  不应在没有参照环境的情况下盲写——这也正是本轮**没有**继续往下写 P2 的原因。

## 下一步（需人拍板）

| 选项 | 做法 | 代价 |
| --- | --- | --- |
| A（推荐先做） | 在**能跑通导入的真实产品环境**上抓参照：跑 packaged 流程，记录 helper 的帧、reason 与耗时 | 需一台真机 + 一次真实操作 |
| B | 若 A 证明需要注入：把「注入 + 通道」当独立 spike 立项（含安全评审），不混进 M0 收尾 | 工作量与风险都显著 |
| C | 先不碰解封，维持 C# 在位 | 功能保持现状；Rust 版停在 P1 |

**明确不做**：把 P2 实现成「无解封能力」就替换 C#——那会把功能从「可能可用」变成「确定不可用」。
