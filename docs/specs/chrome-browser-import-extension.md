# Spec：通过 Chrome 扩展导入浏览器数据（P1）

状态：**草案**（2026-10-18）。上游依据：
`docs/plans/2026-10-17-chrome-abe-import-spike.md`（T1 最终结论——受支持扩展在 Medium 完整性
会话下读到 34/34 条 v20 cookie）、`docs/specs/chrome-abe-import-security.md`（R1–R10）、
`docs/specs/browser-import-helper-contract.md`（现有 helper 契约，P1 不改变它）。

## 1. 目的与范围

用**受支持的 Chrome 扩展 + Native Messaging** 把当前用户 Chrome 的 cookie（含 app-bound v20）
导入 Mode 的内置浏览器会话，替代/并行于现有 app-bound helper 路径。

**不做**：不注入 Chrome 进程、不读密码/历史/支付数据、不把 cookie 或密钥发到网络、
不持久化导入授权。

## 2. 产品规则

1. **逐次用户动作**：每次导入都由用户在扩展 popup（或 Mode 设置页）点击触发；扩展不在后台
   自动读取 cookie。
2. **显式安装**：用户必须显式安装扩展；Mode 检测不到扩展时给出安装指引，不静默降级。
3. **数据最小化**：只发送本次导入需要的 cookie 字段（name/value/domain/path/secure/httpOnly/
   sameSite/expirationDate/hostOnly）；不发送密码、历史、书签、LocalStorage 以外的数据。
4. **不落盘敏感物**：cookie 值只在扩展内存、Native Messaging 管道与 Mode 进程内存中存在；
   日志只记数量与 reason，不记值。
5. **单次授权**：不保存“已允许导入”的全局状态；下次导入重新由用户点击。
6. **失败可见**：扩展缺失、Native host 缺失、用户取消、消息过大、Chrome 重启等都要有稳定
   错误码与用户可读提示。

## 3. 状态所有者与数据流

```text
Chrome 扩展（无持久状态）
  用户点击 popup「导入到 Mode」
    → chrome.cookies.getAll（受支持 API，Chrome 自己解密 v20）
    → chrome.runtime.sendNativeMessage("com.mode.browser_import", {v, cookies})
Native host（Rust，单次进程，无持久状态）
    → 校验消息形状/大小/来源
    → 通过命名管道把 payload 交给 Mode main（一次性 token）
Mode main（导入会话所有者）
    → 校验 token 与 payload
    → 写入内置浏览器 session（现有 import 逻辑）
    → 只返回计数与 reason
```

- **扩展**不持有授权状态；**Native host** 不落盘；**Mode main** 持有本次导入会话与结果。
- 三者的边界与错误码是接口契约，不允许互相猜测。

## 4. 接口

### 4.1 扩展 ↔ Native host（Chrome Native Messaging）

- 传输：4 字节小端长度 + UTF-8 JSON（Chrome 规定）。
- 单条消息上限 **1 MiB**；超过由扩展分片或拒绝（实现计划里定分片）。
- 请求：
  ```json
  { "v": 1, "kind": "cookies", "cookies": [
      { "name": "...", "value": "...", "domain": "...", "path": "/",
        "secure": true, "httpOnly": true, "sameSite": "lax",
        "expirationDate": 1234567890, "hostOnly": false } ] }
  ```
- 响应：`{"ok": true, "count": 34}` 或 `{"ok": false, "error": "<稳定码>"}`。
- 取消请求（popup 在导入完成前关闭时尽力发送）：
  ```json
  { "v": 1, "kind": "cancel" }
  ```
  响应：`{"ok": true}`。主流程仍以自身 TTL 兜底，不依赖这条消息必达。

### 4.2 Native host ↔ Mode main

- Windows 命名管道：`\\.\pipe\mode-browser-import-<random>`；管道名与一次性 token 由 Mode main
  写入 `%LOCALAPPDATA%\Mode\browser-import\channel.json`（ACL 仅当前用户）。
- 传输：同样的长度前缀 JSON；host 先发 token 握手，main 回 `ready`，再发 payload。
- 失败码：`channel_missing` / `channel_token_mismatch` / `channel_timeout` / `payload_too_large`。

### 4.3 Mode main

- 复用现有 `importChromeCookies` 的写入路径（`electron.Session.cookies.set`）。
- 与现有 helper 路径互斥：P1 成功即不再调用 app-bound helper；失败时可回退到现有路径
  （但不得在不告知用户的情况下静默回退）。

## 5. 失败语义（稳定码）

| 场景 | 码 |
| --- | --- |
| 扩展未安装 / 未启用 | `extension_missing` |
| Native host 未注册 / 启动失败 | `native_host_missing` |
| 用户关闭 popup / 取消 | `import_cancelled` |
| 消息超过上限且无法分片 | `payload_too_large` |
| 管道不存在 / token 不匹配 | `channel_missing` / `channel_token_mismatch` |
| 通道 TTL 内没有任何连接 | `extension_missing` |
| 已连接但迟迟没有合法帧 | `channel_timeout` |
| payload 形状不合法 | `invalid_payload` |
| 写入内置浏览器失败 | `cookie_write_failed` |

## 6. 安全要求

必须逐条满足 `docs/specs/chrome-abe-import-security.md` 的 R1–R10。本 spec 追加：

- 扩展的 `host_permissions` 与 `cookies` 权限在安装页可见；不申请无关权限。
- Native host 只信任 `allowed_origins` 中的扩展 ID；对 payload 做大小、字段类型与域名格式校验。
- 管道 token 一次性、短 TTL（≤60s）、仅当前用户可读；导入完成即删除 channel.json。
- 发布构建对 Native host 做签名/哈希校验；失败 fail-closed。
- 日志不出现 cookie value / token / 完整 payload。

## 7. 分发与安装（待拍板）

- **默认建议**：企业内部/侧载（enterprise policy 或 developer mode 安装），先不做 Web Store。
- 需要拍板：Web Store 上架 vs 仅企业策略；扩展 ID 固定方式；Native host 由安装器注册还是
  Mode 首次导入时引导注册（后者需要写 HKCU，权限较低）。
- 无论哪种方式，都必须有「扩展缺失」的可操作指引。

## 8. 验收场景

1. Chrome 155、Medium 完整性、真实 profile（34 条 v20）：点击导入 → Mode 内置浏览器获得
   34 条 cookie；计数一致。
2. 用户拒绝/关闭 popup → `import_cancelled`，不写入任何 cookie。
3. 扩展未安装 / Native host 未注册 → 对应稳定码与安装指引。
4. payload 超 1 MiB → `payload_too_large`，不写入。
5. Chrome 重启后再次导入 → 无持久授权，仍需用户点击。
6. 日志/错误路径不含 cookie 值与 token。

## 9. 开放决策

- 分发渠道（Web Store / 企业策略 / 侧载）。
- 是否同时导入 LocalStorage（本 spec 先只做 cookie）。
- 大 profile 的分片策略（1 MiB 上限）。
- 与现有 app-bound helper 的长期关系（P1 稳定后是否删除 helper/C#）。
