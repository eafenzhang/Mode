# Spec：Windows 浏览器数据导入 helper 的可观测契约

前置：路线图 `docs/plans/2026-10-09-tauri-rewrite-roadmap.md` M0 任务 6（把 1561 行 C# 换成 Rust）。
纪律：`docs/specs/clean-room-policy.md`。

## 为什么需要这份 spec

被测实现的 C# 源码（`packages/desktop/native/windows-browser-import-helper/Program.cs`）与
`upstream/zcode-baseline` 逐文件 diff **只差 1 行**（1 insertion / 1 deletion），按
`clean-room-policy.md`「识别口径」属于**上游衍生的实现文件**，**禁读**；「翻译、对照改写、
按上游思路复现」明确禁止。

因此 Rust 重写的合法输入只有两类：

1. **黑盒行为**（policy §黑盒对拍：运行旧实现，观察输入输出与事件序列）；
2. **接口面**（policy §允许输入：宿主侧常量、协议帧结构、断言的输入输出）。

本文把这两类输入固定下来，作为 `crates/browser-import-helper` 的唯一取材来源。
本文不含、也不得加入旧实现内部逻辑的描述。

## 三个令牌（实测，2026-10-15）

宿主 `packages/desktop/src/main/windowsChromeAppBoundKey.ts` 定义了两个**不同**的常量，
加上载体字形，共涉及三个字符串：

| 用途 | 取值 | 出处 |
| --- | --- | --- |
| broker **请求/响应**帧令牌 | `MODE_BROWSER_IMPORT_V1` | 宿主常量 `HELPER_PROTOCOL`（14 行），响应断言在同文件 391 行 |
| `--version` 横幅第一字段 | `MODE_BROWSER_IMPORT_HELPER` | 宿主常量 `HELPER_VERSION_PROTOCOL`（15 行），断言在 369 行 |
| 协议版本 | `2` | 宿主常量 `HELPER_VERSION`（16 行） |

**已存在的功能缺陷（实测证据）**：本仓库现役 C# 产物对两者都发 `ZCODE_*` 品牌
（`--version` 打印 `ZCODE_BROWSER_IMPORT_HELPER`，回帧用 `ZCODE_BROWSER_IMPORT_V1`），
与宿主两条断言都不符——即 Windows 下 Chrome app-bound 密钥导入在当前代码里必然被判
`invalid_response`。Rust 版按**宿主契约**（上表）实现以修正该缺陷，不复制该缺陷；
`tests/protocol.rs` 有反向锁（`rejects_legacy_zcode_token`、`responses_never_use_the_version_token`）。

## 契约 A：版本查询（`--version`）

实测（`packages/desktop/bundled-tools/win32-x64/browser-import/mode-browser-import-helper.exe`，
由 `pnpm --filter @mode/desktop prepare:browser-import-helper` 产出）：

```
$ mode-browser-import-helper.exe --version
ZCODE_BROWSER_IMPORT_HELPER	2	x64	0.0.1	4e1800d3
```

制表符分隔 5 字段：`<横幅令牌>\t<协议版本>\t<架构>\t<应用版本>\t<git 修订>`；退出码 0。

- 架构字段取 `x64`／`arm64`（不是 `x86_64`）。
- 第 4/5 字段是构建期注入（应用版本、git 修订），非运行时探测；对拍只比前 3 字段。
- 无参数、`--help`、未知参数：退出码 **2**，无 stdout。

## 契约 B：broker 模式（`--broker --parent-pid <pid>`）

调用面：宿主经 `spawn(helperPath, ["--broker", "--parent-pid", String(process.pid)])` 启动，
`stdio: ["pipe","pipe","ignore"]`（**stderr 被丢弃**，不是契约），stdin 写一行请求后读 stdout。

**请求（stdin，一行，以 `\n` 结尾，制表符分隔 3 字段）：**

```
MODE_BROWSER_IMPORT_V1\t<base64(appBoundKey[4:])>\t<base64(chromeExecutablePath)>\n
```

- 字段 2 是 Chrome `Local State` 的 `os_crypt.app_bound_encrypted_key` 解 base64 后**去掉前 4 字节**
  的密文（宿主另有断言：前缀必须是 ASCII `APPB`）；
- 字段 3 是 Chrome 可执行文件绝对路径（UTF-8 → base64）。

**响应（stdout，一行，制表符分隔 3 字段，令牌为 `MODE_BROWSER_IMPORT_V1`）：**

| 形态 | 字段 | 含义 |
| --- | --- | --- |
| 成功 | `MODE_BROWSER_IMPORT_V1\tOK\t<base64(32 字节密钥)>` | 宿主按 base64 解出 32 字节；长度 != 32 视为 `invalid_response` |
| 失败 | `MODE_BROWSER_IMPORT_V1\tERR\t<reason>` | reason 见下 |

字段数 != 3 或字段 1 不在 `{OK, ERR}` → 宿主判 `invalid_response`；`OK` 形态另要求退出码 0。

**失败并不保证有帧**。实测（已提权、交互式会话，伪密文 + 不存在的 Chrome 路径）：

| 情形 | 退出码 | stdout |
| --- | --- | --- |
| 无参数 / 未知参数 | 2 | 空 |
| 畸形帧、缺字段、**空 stdin** | 1 | **空**（不发 ERR 帧） |
| `--parent-pid 0`（其余合法） | 1 | `…V1\tERR\thelper_failed` |
| 合法帧 + 存活 pid（后续步骤失败） | 1 | **空** |
| **真实 app-bound 密钥 + 真实 chrome.exe + Chrome 155 已运行** | 0 | `…V1\tERR\tbroker_initialization_failed` |

最后一行是 2026-10-15 在装有 Chrome 155.0.8059.80 的机器上的实测：**旧产物在该环境下也进不到成功路径**。
因此本契约的**成功路径在这台机器上无法用旧产物对拍**，验收改用自证口径（见
`docs/plans/2026-10-15-browser-import-abe-spike.md`）：取回 32 字节密钥后解开 Chrome `Cookies`
库中的真实 AES-256-GCM 密文即为通过。

即：**只有部分失败路径以 ERR 帧上报**，其余为静默非零退出。宿主对空 stdout 一律判
`invalid_response`。Rust 版对「能判定为契约内失败」的情形发 ERR 帧，对「无法判定／尚未实现」
的情形保持静默非零退出（`tests/broker_cli.rs` 锁住这两条）。
另：宿主不做请求令牌校验以外的入参校验——实测把令牌换成 `ZCODE_*` 时旧产物行为不变，
说明旧产物并未校验请求令牌（Rust 版按契约校验）。

宿主侧可区分的 reason 子集（其余一律归并成 `chrome_cookie_app_bound_decryption_failed`）：

| reason | 宿主映射的错误码 |
| --- | --- |
| `elevation_cancelled` | `chrome_cookie_elevation_cancelled` |
| `helper_verification_failed` | `chrome_cookie_helper_verification_failed` |
| 其它 | `chrome_cookie_app_bound_decryption_failed` |

宿主列举的 reason 全集（宿主侧白名单 `APP_BOUND_HELPER_FAILURE_REASONS`，重写时需覆盖）：
`helper_failed`、`broker_initialization_failed`、`controller_handshake_failed`、`service_channel_failed`、
`elevation_failed`、`timeout`、`peer_verification_failed`、`service_failed`、`validation_failed`、
`unsupported_key`、`cng_failed`、`decryption_failed`、`service_cleanup_failed`。

**helper 可对外发出的 reason 全集 = 白名单 13 项 + 上表宿主单独识别的 2 项**
（`elevation_cancelled`、`helper_verification_failed`）= **15 项**。这两项虽不在白名单内，但宿主在
`parseHelperResponse` 里先于白名单判定，helper 必须能发出——否则 UAC 取消路径无法映射到
`chrome_cookie_elevation_cancelled`。落在 15 项之外的 reason 一律不得外露，实现侧应归并成
`helper_failed`（fail-closed）。

宿主超时：90s（`HELPER_TIMEOUT_MS`）；stdout 上限 128KiB（`HELPER_MAX_OUTPUT_BYTES`）。

## 契约 C：宿主-helper 的封装层（不进 helper，重写时保留）

包装态（`isPackaged`）下宿主不直接起 helper，而是经受信任的 Windows PowerShell 5.1 包装：

- 启动前对 helper 与 app 主程序各自做 Authenticode 校验，要求两者签名者证书指纹一致；
- 用 SHA-256 摘要比对 `verifiedHelperHash`，不一致时直接回 `ERR\thelper_verification_failed`，不执行 helper；
- 包装脚本把 stdin 整体读入再管道给 `helper.exe --broker --parent-pid <pid>`。

本节描述**宿主侧**行为（属本仓库 TS 实现，非 C# 实现），保留原样；Rust 版 helper 只负责契约 A/B 两端。

## 不可观测（重写时必须自行实现，不得靠对拍推导）

- 提权（UAC）与提权后服务通道的建立/清理细节；
- CNG／DPAPI 层面的密钥解封步骤与算法选择；
- 具体失败在内部的判定顺序（只约束**对外** reason 取值集合与上述映射）。

这些属「用 Windows 公开 API 与公开资料实现」的范围：实现者可查 Microsoft 公开文档，
不得回读旧实现源码。

## 验收

1. 契约 A 的前 3 字段与旧产物逐字节一致；契约 B 的令牌、字段结构、reason 集合按本文对齐。
2. 对拍 harness：`packages/desktop/tests/browser-import-helper-contract.test.mjs`
   （旧产物缺席时整条 skip，保证 ubuntu 上的 verify job 不被 Windows 专用产物拖红）。
3. 本文档不含旧实现内部逻辑描述（评审项：无「它先……再……」式实现叙述）。
4. 重写 PR 的输入来源可回溯到本文 + 公开文档。
