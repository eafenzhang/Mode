# 远程控制 v3 真机验收清单

状态：2026-11-12 待执行。覆盖配对、远程工作区、远程桌面、传输选路、断线恢复与
安全边界；自动化部分先跑 `pnpm test:remote-v3`，本清单只列必须两台真实设备才能
验证的项。

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

### 1.1 本地模拟（2026-11-12 已执行，13 项通过）

用 `playwright-core` 驱动本机 Chrome（真实 `RTCPeerConnection`），加假 GitHub
Device Flow、内存 remote-server 与轻量桌面 Host，按真人路径操作 `/v3`：

- 未登录不展示「本机 Web 设备」区块；输入服务地址后登录入口仍可见；
- Device Flow 登录、生成 Web 设备、目录发现 Host（在线 / host / desktop）；
- 「加密 RPC」显示 `传输：p2p`，`remote.host.info` 返回设备名/平台/版本；
- 退出后账号保留在「已保存的账号」，复用地址重新登录沿用同一 deviceId；
- 「删除」只移除该地址的账号记录。

未覆盖：工作区隧道与远程桌面需要完整 desktop Host（stdio sidecar + 本机 LAN
服务）；轻量 Host 下返回 `HOST_LOCAL_API_UNAVAILABLE` 属预期。真机验收仍按
下方 2–7 节执行。

## 2. 账号与设备目录

| # | 操作 | 预期 |
| --- | --- | --- |
| 2.1 | 两端分别登录不同账号 | 状态 online；侧边栏/账号弹窗显示 v3 账号名与指纹 |
| 2.2 | Host 刷新设备 | 目录只有本账号设备；不含 token/私钥 |
| 2.3 | 退出登录 | 状态 unconfigured；本地 token 清除；目录清空 |
| 2.4 | Web 页面与账号服务不同源（配置 `MODE_REMOTE_ALLOWED_ORIGINS`） | 登录请求的预检与实际响应带回 ACAO；未配置或来源不在白名单时浏览器拒绝读取 |
| 2.5 | `/v3` 依次登录两个不同服务地址，再切换 | 两个账号各自保留身份与登录态；退出其中一个不影响另一个 |

## 3. 跨账号配对（docs/specs/remote-control-v3-pairing.md）

| # | 操作 | 预期 |
| --- | --- | --- |
| 3.1 | Host 生成配对码 | 出现一次性短票；180s 内有效 |
| 3.2 | Client 粘贴配对码 | 显示 8 位验证码与 Host 名称 |
| 3.3 | Host 输入错误验证码 5 次 | 会话作废，提示错误次数过多 |
| 3.4 | 重新配对并输入正确验证码 | Host 确认成功；Client 自动领取新凭证；两端目录互见 |
| 3.5 | 用旧凭证发起连接 | 401/unauthorized，不进入静默重试 |
| 3.6 | 取消配对后 Client 再 claim | 拒绝并提示已结束 |

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
| 5.1 | `/v3` 或桌面端进入桌面 | Host 本机弹出同意窗；拒绝/超时返回 consent_denied/consent_timeout |
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
| 7.4 | 跨账号访问未配对设备 | 403/unauthorized；不泄露设备是否存在之外的业务状态 |
| 7.5 | 撤销 membership 后连接 | 现有连接被关闭（4004）；再次连接要求重新配对 |

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
- 二维码未实现，配对使用短票 + 手动输入验证码。
