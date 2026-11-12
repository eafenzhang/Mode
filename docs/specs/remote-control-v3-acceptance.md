# 远程控制 v3 真机验收清单

状态：2026-11-12 真机首轮（§10）、2026-11-13 第二轮（§11）已完成；键鼠输入、
移动端触控、TURN/跨 NAT 待补。覆盖账号设备目录、远程工作区、远程桌面、传输选路、
断线恢复与安全边界；自动化部分先跑 `pnpm test:remote-v3`，本清单只列必须两台真实
设备才能验证的项。

## 1. 环境准备

- 两台设备：Host（桌面端，账号 A）、Client（桌面端或浏览器 `/v3`，账号 B）。
- 账号服务：`startRemoteServer` 实例，URL 使用 HTTPS（浏览器端必须安全上下文）。
- 两端版本一致；Host 已开启本机 LAN 服务（远程桌面/工作区的本机 API 依赖它）。
- WebRTC 验收：建议部署 coturn 并配置 `MODE_REMOTE_TURN_URLS` /
  `MODE_REMOTE_TURN_SECRET`（见 [fnos-fpk-packaging.md](./fnos-fpk-packaging.md)）；
  仅测同局域网可只配 STUN 或留空（主机候选即可连通）。
- 记录：两端 deviceId、指纹（前 16 位）、服务地址、时间。

自动化前置：

```sh
pnpm test:remote-v3     # core + remote-server + ui + desktop 全量
pnpm typecheck && pnpm lint && pnpm architecture:check --changed
```

### 1.1 本地模拟（2026-11-12 已执行，26 项通过）

可重复执行：先启动 `pnpm --filter @mode/web exec vite --port 5199 --strictPort`，
再运行 `pnpm accept:remote-v3-web`（`--serve` 可常驻供人工点选；需要本机 Chrome，
8787/8788 空闲）。脚本：`scripts/acceptance/remote-v3-web.mts`。

用 `playwright-core` 驱动本机 Chrome（真实 `RTCPeerConnection`），加假 GitHub
Device Flow、两套内存 remote-server 与轻量桌面 Host，按真人路径操作 `/v3`：

- 未登录不展示「本机 Web 设备」区块；输入服务地址后登录入口仍可见；
- Device Flow 登录、生成 Web 设备、目录发现 Host（在线 / host / desktop）；
- 「加密 RPC」显示 `传输：p2p`，`remote.host.info` 返回设备名/平台/版本；
- 「工作区隧道」经 Host 本机 `/ws/host` 桥接到真实 HTTP/WS 服务，
  `systemService.info` 返回预期数据（P2P 数据面）；200KB 响应尾部标记完整返回
  （验证 Noise 分片与 DataChannel 分片）；
- 禁用浏览器 WebRTC 后自动回落 `relay`，同一 RPC 仍成功；
- 两个服务地址（两个账号）各自保留身份；切回账号 A 时目录只显示 A 的 Host；
- 退出后账号保留在「已保存的账号」，复用地址重新登录沿用同一 deviceId；
- 「进入桌面」在 Host 无桌面服务时显示错误层与「返回设备列表」，不白屏；
- Host 下线后刷新目录显示「离线」且三个操作按钮禁用；Host 恢复后回到「在线」；
- 「删除」只移除该地址，另一账号不受影响；退出时清空旧错误提示；
- 390×844 手机视口无横向滚动，P2P RPC 仍可用；
- 不可达服务地址返回可见错误（`Failed to fetch`），不卡在加载态。

仍未覆盖：远程桌面键鼠输入/画质切换/断网重连、移动端触控/软键盘、TURN/跨 NAT。
真机验收进展见 §10。

## 2. 账号与设备目录

| # | 操作 | 预期 |
| --- | --- | --- |
| 2.1 | 两端分别登录不同账号 | 状态 online；侧边栏/账号弹窗显示 v3 账号名与指纹 |
| 2.2 | Host 刷新设备 | 目录只有本账号设备；不含 token/私钥 |
| 2.3 | 退出登录 | 状态 unconfigured；本地 token 清除；目录清空 |
| 2.4 | Web 页面与账号服务不同源（配置 `MODE_REMOTE_ALLOWED_ORIGINS`） | 登录请求的预检与实际响应带回 ACAO；未配置或来源不在白名单时浏览器拒绝读取 |
| 2.5 | `/v3` 依次登录两个不同服务地址，再切换 | 两个账号各自保留身份与登录态；退出其中一个不影响另一个 |

## 3. 跨账号配对（已下线）

2026-11-12 起取消跨账号配对，统一个人账号体系：两端使用同一 GitHub 账号登录即可在
设备目录互见并直接连接，不需要配对码。本节不再执行。

## 4. 远程工作区

| # | 操作 | 预期 |
| --- | --- | --- |
| 4.1 | Client 从连接向导选 Host | 建立 Noise 连接并注册远程 session |
| 4.2 | 打开远程 tab | 文件树/终端/Git/设置来自 Host；本地操作不受影响 |
| 4.3 | 拔网/断 Host 网络 10s 后恢复 | 隧道自动重开；同一 tab 内 RPC 恢复，无需重新连接 |
| 4.4 | 关闭 tab | Host 侧连接数回落；无残留 `/ws/host` 隧道 |
| 4.5 | Host 退出登录后 Client 发起 | 明确失败（非无限等待） |

## 5. 远程桌面

| # | 操作 | 预期 |
| --- | --- | --- |
| 5.1 | 同账号 v3 设备进入桌面 | 不弹同意窗：同账号设备（Noise 设备身份 + 账号凭证）视作已授权客户端（`cloud-account-peer`，见 cloud-personal-accounts.md「同账号即信任」），`rd_consent_authorized` 直通；LAN 未配对/未记住设备仍弹同意窗，拒绝/超时返回 consent_denied/consent_timeout |
| 5.1a | LAN 未配对设备进入桌面 | Host 本机弹出同意窗；拒绝/超时返回 consent_denied/consent_timeout；「记住设备」后直通 |
| 5.2 | 同意后首帧 | ≤5s 出画；状态条有 fps/RTT/分辨率 |
| 5.3 | 键鼠输入 | Host 端正确响应；浏览器内焦点保持在画面 |
| 5.4 | 切换画质档位 | bye + 重建会话；不二次弹同意窗 |
| 5.5 | 媒体流断开（断网 5s） | 画面提示重连；恢复后继续出画，不重建整个会话 |
| 5.6 | 关闭会话 | Host 释放采集；无残留媒体/信令连接 |
| 5.7 | 浏览器缺少 WebCodecs（如非安全上下文） | 发起前返回 `decoder_unsupported`，不弹同意窗 |

## 6. 断线与恢复

| # | 操作 | 预期 |
| --- | --- | --- |
| 6.1 | Control 断链 30s 后恢复 | Host/Client 按退避自动重连；状态回到 online |
| 6.2 | Control 断链期间发起 RPC | 在途请求立即失败，不等到超时；重连后新请求成功 |
| 6.3 | Host 重启 | Client 侧 Control 重连；既有 remote session 标记断开，可重新连接 |
| 6.4 | 刷新浏览器 `/v3` 页面 | 账号与设备身份恢复（IndexedDB）；远程桌面/工作区会话不跨页面恢复 |
| 6.5 | 同局域网两台设备建连（未配 STUN/TURN） | 选路为 `p2p`；RPC/桌面/工作区正常，Relay 无业务密文以外的流量 |
| 6.6 | 配置 coturn 后跨 NAT 建连 | 选路为 `p2p`；抓包可见 TURN 中继但内容仍为 Noise 密文 |
| 6.7 | 停掉 coturn 或封锁 UDP 后建连 | 8s 内回落 `relay`，业务可用；日志有 p2p 失败原因 |
| 6.8 | 浏览器 `/v3`（支持 RTCPeerConnection）连桌面 Host | 连接测试结果首行显示 `传输：p2p`；主动禁用 WebRTC（或 `p2p:false`）后显示 `relay` |
| 6.9 | P2P 建立后断网 10s 再恢复 | 数据面断开触发重连；恢复后重新协商，RPC 不重放 |

## 7. 安全边界

| # | 检查 | 预期 |
| --- | --- | --- |
| 7.1 | 抓包 Relay 数据面 | 只有 Noise 密文，无业务方法名/参数 |
| 7.2 | 查看浏览器 IndexedDB | profile 为 `{v,iv,data}` 信封，不含私钥/token 明文（IndexedDB 分支） |
| 7.3 | 强制 localStorage 回落（隐私模式） | profile 明文；规格已声明，记录为已知限制 |
| 7.4 | 访问其他账号的设备 | 403/unauthorized；不泄露设备是否存在之外的业务状态 |
| 7.5 | 撤销 membership 后连接 | 现有连接被关闭（4004）；再次连接被拒绝，需重新登录/授权 |

## 8. 记录模板

```text
日期：
版本/commit：
Host：设备名 / deviceId / 指纹：
Client：设备名 / deviceId / 指纹：
服务地址：
失败项：# 编号 + 现象 + 日志时间点 + 是否可复现
```

## 9. 已知限制（验收时按此判定，不算失败）

- LAN 监听默认仍是 WS；配置 `MODE_LAN_TLS_KEY_FILE`/`MODE_LAN_TLS_CERT_FILE` 后为
  HTTPS/WSS（证书须受信任，自签名不支持）。跨公网仍回落 Relay。
- 浏览器 LAN direct 受混合内容限制（HTTPS 页面不能连 `ws://`），自动回落 Relay。
- 浏览器 `localStorage` 回落路径的凭据不加密。
- TURN 需自建 coturn（REST 临时凭证）；未配置或 UDP 不通时回落 Relay，
  跨对称 NAT 可能只能走 Relay。
- 浏览器不支持 `RTCPeerConnection`（或非安全上下文限制）时只走 Relay。
- 跨账号配对已下线；不同账号设备之间不再互通。

## 10. 真机验收记录（2026-11-12，Windows 桌面端 Host + Chrome Client，线上 remote.mymode.cloud）

环境：Host = dev 构建桌面端（账号 eafenzhang，deviceId 43c1ba78…，
指纹 a295692d…，LAN 100.90.96.50:45880）；Client = Chrome 持久 profile
（deviceId 271953fe…）；服务地址 https://remote.mymode.cloud。独立 Web 入口为
`https://rc.mymode.cloud`（首轮验收时尚为 mymode.cloud/v3，现已切换），
生产 OAuth 登录已在 rc 域名复验通过（新设备 a5cd405f…，2026-11-13）。

已通过：

- §2.1/2.2：Host 上线，账号名/指纹可见；Web 目录只显示同账号 Host，不显示本机 Web 设备。
- §6.8：加密 RPC `传输：p2p`，`remote.host.info` 返回真实设备信息。
- §4.1/4.2（连通面）：工作区隧道 `传输：p2p`，`systemService.info` 返回 Host 数据。
- LAN direct（禁用 WebRTC 强制走 LAN）：RPC 与工作区均 `传输：lan`，真实命中
  Host `/ws/v1/direct`（HTTPS 页面需 wss 地址，本次为 HTTP 页面 + 明文 LAN 地址）。
- §5.1：同账号 Web 设备进入桌面**不弹同意窗**（按 5.1 修订口径，`cloud-account-peer`
  已授权直通）；§5.2：首帧 ≤7.5s 出画，1920×1080，45–90 fps，码率/延迟正常。
- §5.6：断开后回到设备列表，Host 释放会话，可再次建连。

遗留（未执行/未通过）：

- §5.3：键鼠输入（会移动真实指针，未执行）。
- §6.6/6.7：TURN/跨 NAT（coturn 未部署）。
- 手机真机（触控/软键盘）。

首轮发现的缺陷与处置：

- 远程桌面状态条端到端延迟显示 `~1.79e12 ms`：采集戳 u32 截断被直接当 Unix 毫秒相减；
  已修复为按 32 位回绕取差值（`computeFrameAgeMs`，偏移失准返回 null 不进统计）。
- Host 凭证被服务端 4002 拒绝后无限退避重连（access/refresh 均失效时不进错误态）：
  已修复为 4002 触发 refresh 单飞恢复，成功重连、401/403 进错误态等重新登录。
- 同意窗行为按 5.1 修订口径确认为设计预期，不改代码。

## 11. 第二轮真机验收记录（2026-11-13，Host = dev 桌面端，Client = Chrome）

环境同 §10；Host 用当前源码 sidecar（含 4002 恢复修复），Client 为已登录的
Chrome 持久 profile（deviceId 271953fe…），服务地址 https://remote.mymode.cloud。

补充通过：

- §5.4 画质切换：清晰 1920×1080 → 流畅 1600×900（90 fps）→ 清晰，**会话不断开、
  不二次弹同意窗**；Host 侧日志为 bye + 新建 media relay（`consent requested →
  consent allowed, remember=true` 由同账号策略自动放行）。
- §5.5 媒体断线：浏览器 `setOffline(true)` 6s 后恢复，画面回到「画面已连接」并继续
  出画（90+ fps），会话未整体重建。
- §6.1 Host 侧 Control 断开：服务端重启（4009 server shutdown）后 Host 自动退避
  重连回 online，无需重启应用。
- 4002 真实过期凭据：把安装版 0.0.124 的 access token 放到过期后启动 dev sidecar，
  复现「连接被 4002 拒绝」；修复后的日志只出现一次 4002、随后 refresh 成功并回到
  online（`last_seen` 恢复），不再无限退避。
- §7 安全边界（新增 CSP）：`https://rc.mymode.cloud` 响应带 CSP 与
  `X-Content-Type-Options`/`Referrer-Policy`/`Permissions-Policy`；生产构建在该 CSP
  下目录、远程桌面与媒体链路均正常，控制台 0 条 CSP 违规。

本轮观察（不判定失败，记录风险）：

- 隧道下行实测约 140 KB/s，旧入口（整产品包）首屏 DOMContentLoaded 达 88s；已改为
  `v3.html` 独立轻量入口（初始 10 个文件、约 250KB gzip），实测 2.7–3.8s。
- 远程桌面会话面仍为按需 4.97MB（gzip 1.44MB）独立 chunk，弱网首进桌面仍偏慢。
- 另一台在线 Host（MX121212）在握手阶段被服务端以 4001 关闭，Host 只回 p2p.answer、
  不回 `secure.handshake`；怀疑该机器运行旧版本，待其升级后复测。
