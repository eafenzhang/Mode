# 远程桌面（系统 RDP / mstsc 直连）

状态：已拍板（2026-10-15）。**本文是本功能的唯一权威**，取代 `lan-remote-desktop.md`（已收敛为指向本文的存根）与已删除的 `lan-remote-desktop-native-media.md`。

关联：[lan-remote-attach.md](./lan-remote-attach.md)（LAN 配对、令牌与握手时序）、[lan-discovery.md](./lan-discovery.md)（设备发现）、[lan-paired-devices.md](./lan-paired-devices.md)（已配对设备管理）、[lan-host-server-mode.md](./lan-host-server-mode.md)、[lan-remote-computer-use.md](./lan-remote-computer-use.md)（模型驱动的远端电脑控制，与本功能互补且与之无代码耦合）、[clean-room-policy.md](./clean-room-policy.md)。

## 背景与决策

2026-10-12 起本功能是自研媒体面：被控端 DXGI Desktop Duplication 采集 + Media Foundation 硬编 → helper 逐帧 AES-256-GCM → 本机 TCP → 被控 host 中继 → LAN WebSocket → 主控 WebCodecs 解码画 canvas，输入反向上行注入。该实现已实机跑通并逐项修完黑屏/光标/异步 MFT 等根因。

2026-10-15 对照 Windows RDP 评估后决定**改用它**：RDP 在局域网下延迟与带宽效率本就不落后，而自研面永远补不齐的能力（剪贴板、音频重定向、多显示器同时铺开、Ctrl+Alt+Del 安全注意序列、文件/打印机重定向、带宽自适应与断线重连）是系统自带。应用因此**不再是任何远控协议端点**，退化为「设备发现 + 配对 + 一键发起 + 会话管理」的启动器。

### 得到的

剪贴板、音频重定向、多显示器同时铺开、Ctrl+Alt+Del（SAS）可用、文件与打印机重定向、RDP 自身的带宽自适应与断线重连；自有编解码/传输/注入链路（约 5,200 行 + 其测试）整体消失，维护成本归零。

### 让渡的（永久性产品语义变更）

| 原能力 | 变更后 |
| --- | --- |
| 被控端支持 Windows 家庭版 | **不可被控**：RDP 主机需 Pro/Enterprise |
| 本地用户与远端共享输入（原产品规则 4） | **作废**：RDP 接管会话，被控端本地用户被踢下线 |
| 应用层同意窗 + 进行中指示条 | **作废**：Windows 凭据登录即同意，应用无插入点 |
| 应用层可拦截连接 | **作废**：3389 暴露即任何可达且持凭据者均可连，应用无法阻断 |
| 应用内会话面（画布/操作栏/状态条） | **删除**：画面在独立 mstsc 窗口 |
| 画质档位 / 码率自适应 / 分辨率协商 | 交给 RDP（`networkautodetect` / `bandwidthautodetect` / 动态分辨率） |
| 被控中不可发起（原产品规则 9） | **作废**：本机被 RDP 连接时进程状态不可靠，不做判定 |

## 范围与非目标

范围内：两端均为 Windows 的 Mode desktop，经 LAN（含 Tailscale）直连；用系统 RDP 完成画面、输入、剪贴板、音频、多显示器；设备发现与配对沿用既有链路；被控端系统 RDP 状态检测与开启指引。

非目标：macOS/Linux 对端；手机 Web 端；公网中继与 NAT 穿透；应用内画面渲染（不嵌入 mstsc 窗口）；应用层同意/授权闸门；一键开启系统 RDP（需提权，见产品规则 5）；自研 RDP 协议实现；引入第三方远控实现。

## 产品规则

1. **发现与归属仍走既有 LAN 配对**：设备列表 = 已配对设备（`getLanPairedPeers`）+ 未配对检测（`discoverLanPeers`）+ 配对码流程。配对不参与 RDP 认证（认证由 Windows 凭据承担），只决定「设备页展示哪些设备」。
2. **连接 = 启动系统 RDP 客户端**：主控点「进入桌面」→ main 生成临时 `.rdp` → `spawn mstsc.exe <file>`。会话在独立 mstsc 窗口，应用不渲染画面、不采集输入。
3. **凭据**：`.rdp` 只写 `username`（可选，用户在连接选项里填）；**密码不写**（模板不含 `password 51:b:`），由 Windows 自己的登录框收集，并用系统「记住我」记忆（存 Credential Manager、DPAPI 保护）。应用不存储任何密码。
4. **被控端只检测、不改系统设置**：设置页显示系统 RDP 状态（`fDenyTSConnections` 与 Windows 版本是否支持）与可复制的开启指引；**不做一键开启**（改注册表与防火墙规则需 UAC，不引入提权通道）。
5. **提示性共享开关**：保留单一开关「在本应用内把本机共享为可远控设备」，UI 必须明写**「不影响系统远程桌面」**——它只决定本机是否作为可远控设备出现在对端设备页（能力位），不具备安全闸门作用。
6. **逐设备授权开关删除**：系统 RDP 下没有执行点（授权发生在 Windows 登录，不在本应用），保留即为假控制。
7. **能力位**：`server-info.capabilities.remoteDesktop` = win32 && 提示性开关 && kill-switch（沿用旧口径，作为「本机愿意被远控」的声明）；新增 `systemRdp: { enabled: boolean; editionOk: boolean }` 报告系统侧真实状态，握手即时计算（读注册表），无持久化。
8. **被控端能力门（设备卡置灰判据，自上而下命中即返回）**：
   1. `remoteDesktop === false`（对端未共享为可远控设备）→ 置灰「对端未共享为可远控设备」；
   2. `systemRdp.editionOk === false` → 置灰「对端为家庭版，系统远程桌面不可用」；
   3. `systemRdp.enabled === false` → 置灰「对端未开启系统远程桌面」+ 指引。
   **共享位必须参与置灰**：应用拦不住 3389，但主控不该无视对端机主的共享意愿；该位若不参与，
   被控端那个开关在 UI 上就等于不存在（假开关）。`systemRdp` 缺键（旧对端/探测未就绪）**不置灰**——
   状态未知不等于不可用，真不支持时由 mstsc 自身报错。
9. **连接选项（主控侧，按设备记忆）**：全屏 / 多显示器 / 剪贴板重定向 / 音频重定向 / 用户名 / RDP 端口（默认 3389）。默认：窗口模式、剪贴板开、音频带到本机。

## 状态所有者

| 状态 | 唯一所有者 | 说明 |
| --- | --- | --- |
| 提示性共享开关 | 被控 host（凭据存储 `remote-desktop:server-enabled`） | 设置页写；能力位现读 |
| 系统 RDP 状态（注册表读值） | 被控 host（`systemRdpState` 探测模块） | 带 TTL 缓存；不缓存到磁盘 |
| 配对与设备令牌 | 既有 lanAccess 凭据服务 | 不变 |
| 连接选项（每设备） | 主控 main（凭据存储 `rdp:options:<serverId>`） | 设备页/连接弹窗写；启动器读 |
| 已启动的 RDP 会话 | 主控 main 内存（`rdpLauncher`） | 进程句柄与 pid；不持久化（见已知限制） |
| `.rdp` 临时文件 | 主控 main | `<temp>/mode-rdp/<sessionId>.rdp`，会话结束与应用退出即删 |
| 画面/输入/剪贴板/音频 | **Windows RDP**（应用不持有） | 应用零参与 |

## 架构

```text
被控端应用（仅两件事）                      主控端应用
  server-info.capabilities:                    设备页（LAN 发现 + 配对，不变）
    remoteDesktop: win32 && 开关 && !killSwitch    └─ [进入桌面]
    systemRdp{enabled,editionOk} ← reg.exe 现读          ├─ 读/写每设备连接选项
  提示性开关 ← 凭据存储                                 ├─ buildRdpFile(options) → 临时 .rdp
  （无会话逻辑 / 无同意窗 / 无采集 / 无注入）            ├─ spawn mstsc.exe <file>
                                                        └─ child.on("exit") → 会话列表移除
  网线/局域网 ────────────────────────────────────►  Windows RDP (3389)
  LAN 配对与令牌：仅用于发现与设备归属，与 RDP 认证无关
```

被控端应用不参与会话；RDP 认证、加密与多路复用全部由 Windows 承担。

## 接口

### 能力位（`packages/shared/src/server-remote.ts`）

```ts
capabilities?: {
  remoteDesktop?: boolean;                                  // 已有：本机愿意被远控（提示性开关）
  systemRdp?: { enabled: boolean; editionOk: boolean };      // 新增
}
```

`systemRdp` 缺键 = 旧对端不报告 → 设备卡按「状态未知」呈现（不置灰，仍允许发起，RDP 自身会给出失败）。

### 注册表探测（被控 host）

- `HKLM\SYSTEM\CurrentControlSet\Control\Terminal Server` → `fDenyTSConnections`（`0` = 已启用）。
- `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion` → `EditionID`（`Core`/`CoreCountrySpecific`/`CoreSingleLanguage` = 家庭版，其余视为支持）。
- 读法：`reg.exe query`（既有先例 `desktopWindowsOpenFolderContextMenu.ts`）。只读 HKLM，**无需提权**。带 TTL 缓存，失败按「未知」降级（不谎报可用）。

### 主控 IPC（`rd/*`，四处同步：main 常量 → preload → shared `IPlatformService` → renderer 桥）

| 通道 | 入参 / 出参 | 说明 |
| --- | --- | --- |
| `rd/get-peer-capability` | `serverId` → `{online, remoteDesktop, systemRdp?, name?}` | 沿用探测链路，载荷改形 |
| `rd/connect` | `serverId, options` → `{ok:true, sessionId}` \| `{error}` | 生成 .rdp 并启动 mstsc |
| `rd/get-sessions` | → `Array<{sessionId, serverId, name, address, pid, startedAt}>` | 仅本进程启动的会话 |
| `rd/disconnect` | `sessionId` → `{ok}` | 终止对应 mstsc 进程 |
| `rd/get-server-state`（被控） | → `{sharingEnabled, systemRdp}` | 设置面读 |
| `rd/set-server-enabled`（被控） | `enabled` → 同上 | 提示性开关写 |

删除：`rd/start-session`、`rd/send-signal`、`rd/end-session`、`rd/session-event`、`rd/get-controlled-state`、`rd/revoke-client`、`rd/authorize-client`。

### `.rdp` 模板（`buildRdpFile`，纯函数，可单测）

固定项（不可由 UI 关闭）：

```
enablecredsspsupport:i:1        不降低认证等级
authentication level:i:2        要求服务器身份验证
prompt for credentials:i:1      始终由 Windows 收集凭据（我们不存密码）
promptcredentialonce:i:1
networkautodetect:i:1           带宽自适应交给 RDP
bandwidthautodetect:i:1
connection type:i:7
bitmapcachepersistenable:i:1
keyboardhook:i:2
compression:i:1
```

可变项：`full address:s:<address>[:port]`、`username:s:<user>`、`screen mode id:i:1|2`、`use multimon:i:0|1`、`dynamic resolution:i:1`、`smart sizing:i:1`、`redirectclipboard:i:0|1`、`audiomode:i:0|1|2`、`redirectprinters:i:0`、`drivestoredirect:s:`（空）。

**地址口径**：`full address` 用设备主机名/IP + RDP 端口（默认 3389），**不是** Mode LAN server 的端口。

### 安全（不可妥协项）

- **`.rdp` 值注入**：`.rdp` 是 `key:type:value` 文本格式，值中含换行即可注入任意选项（如 `alternate shell`）。`full address` 取自发现结果（对端可控）、`username` 取自用户输入，**两者都必须校验**：地址限 IPv4/IPv6/主机名，用户名拒控制字符与 `\r\n`；校验失败拒绝生成文件并回稳定错误码，不静默截断。
- 不写 `password 51:b:`；不降 `authentication level`；不启用 `enablecredsspsupport:i:0`。
- 临时 `.rdp` 放 `<temp>/mode-rdp/`（专用子目录），会话进程退出与应用退出即删；文件不含密码，泄漏面限于地址与用户名。
- 探测只读 HKLM、不写系统设置、不申请提权。
- 会话生命周期（启动/结束/失败）写 info 级日志，含设备名与地址，不含凭据。

## 事件顺序

```text
① 设备与能力（复用既有发现/配对）
   被控 host  createServerInfo.capabilities.remoteDesktop / systemRdp（注册表现读）
   主控设备页 getLanPairedPeers() + discoverLanPeers()；能力位按需拉取

② 连接（主控单边，无握手、无同意窗）
   设备卡「进入桌面」→ rd/connect(serverId, options)
     main：取该设备已存连接选项 → 校验 address/username → buildRdpFile
           → 写 <temp>/mode-rdp/<sessionId>.rdp → spawn mstsc.exe <file>
           → 记入会话表（pid / serverId / address）
   mstsc：Windows 登录框（预填 username）→ 用户输入密码（可勾「记住我」）→ 进入会话

③ 会话中
   画面/输入/剪贴板/音频/多显示器 = Windows RDP，应用零参与
   应用切页/最小化/重启：不影响会话（mstsc 是独立进程）

④ 结束
   路径：mstsc 窗口关闭或连接断开 / 应用内点「断开」（rd/disconnect = 终止该 pid）/
        应用退出（不终止会话，仅停止跟踪）
   → 删除该会话的 .rdp 临时文件 → 会话表移除 → info 日志
```

## 失败语义

| 失败 | 行为 |
| --- | --- |
| 对端 `remoteDesktop=false` | 设备卡置灰：「对端未共享为可远控设备」（提示性开关关 / kill-switch / 旧对端开关关，不猜具体是哪个） |
| 对端 `editionOk=false` | 设备卡置灰：「对端为家庭版，系统远程桌面不可用」 |
| 对端 `enabled=false` | 设备卡置灰 + 指引：「对端未开启系统远程桌面」（附开启步骤） |
| `systemRdp` 缺键（旧对端） | 不置灰，按状态未知呈现；失败由 mstsc 自身给出 |
| 注册表探测失败 | 按未知降级，不谎报可用；写 warn 日志 |
| 地址/用户名校验失败 | 回稳定错误码，拒绝生成 `.rdp`（不截断、不猜测） |
| mstsc 不存在 / 启动失败 | 回错误并清理临时文件；写 warn |
| 凭据错误 / 空密码账号 / 账号无远程登录权 | mstsc 自身报错，应用不解释、不重试（见已知限制） |
| 3389 被防火墙拦截 | mstsc 连接超时；被控端设置页指引含放行步骤 |
| 应用重启 | 不恢复会话跟踪（`rd/get-sessions` 为空）；mstsc 窗口继续存在，可自行关闭 |

## 已知限制

- **被控端需 Windows Pro/Enterprise**；家庭版无法被控（能力位置 `editionOk=false`）。
- **被控端本地登录用户会被踢下线**（RDP 单会话）；不再有「共享输入」模式。
- **应用层无法拦截连接**：只要系统 RDP 已启用且 3389 可达，持凭据者即可连接，与本应用是否运行无关。「共享开关」仅影响设备页展示。
- 空密码账号默认禁止网络登录（Windows 策略），需给账号设密码。
- 微软账号需以 `MicrosoftAccount\<邮箱>` 形式登录（mstsc 登录框通常自动处理；异常时手输）。
- 多显示器：由 RDP 的 `use multimon` 铺开全部屏幕，不再支持「选某一块」。
- 应用重启后不跟踪既有会话；也不在退出时终止会话。
- 会话发生在独立窗口，应用内的会话面（画布/操作栏/状态条）已删除，不再提供应用内全屏与状态读数。

## 验收场景（两台 Windows 实机，用户执行）

- [ ] 家庭版被控端：设备卡显示「系统远程桌面不可用」并置灰，无法发起。
- [ ] 被控端关闭**共享开关** → 设备卡显示「对端未共享为可远控设备」并置灰；重新打开后恢复可连接（开关只影响展示，不影响系统 RDP 本身）。
- [ ] Pro 被控端未开启远程桌面：设备卡显示「未开启」+ 指引；按指引开启（含防火墙）后重新检测变为可连接。
- [ ] 点「进入桌面」→ 生成临时 `.rdp` → mstsc 窗口出现并预填用户名 → 输入密码后进入桌面。
- [ ] 被控端本地用户被踢下线（符合预期）。
- [ ] 剪贴板双向：本机复制文本 → 远控内粘贴；反向亦然。
- [ ] 音频：被控端播放声音能在主控端听到（`audiomode` 默认带到本机）。
- [ ] 多显示器：开启选项后两块屏同时铺开。
- [ ] Ctrl+Alt+Del：在 RDP 会话内可发送（旧自研面不可用）。
- [ ] 应用内点「断开」→ 对应 mstsc 进程结束、临时 `.rdp` 被删除、会话列表移除。
- [ ] 关闭 mstsc 窗口 → 会话列表自动移除、临时文件删除。
- [ ] 应用退出：mstsc 会话不被终止；临时 `.rdp` 被清理。
- [ ] 地址/用户名注入尝试（含换行）被拒绝并回稳定错误码，不生成 `.rdp`。
- [x] `pnpm typecheck` / `pnpm lint` / `pnpm architecture:check --changed` 全绿（2026-10-15 本地实测，见「实现进度」）。

## 分期

- **P1（本 spec）**：能力位与检测、启动器与 `.rdp` 生成、IPC 面、设备卡连接、会话列表、被控端设置面、i18n；删除自研媒体管线。
- **P2**：每设备连接选项的完整 UI（当前只暴露规则 9 的六项）、会话跟踪持久化（应用重启后仍可见）、对端 3389 可达性预探测。
- **P3**：不做（不引入提权一键开启、不嵌入 mstsc 窗口、不自研 RDP）。

## 实现进度

- [x] P1 代码落地（2026-10-15）：能力位与检测（`host/systemRdpState.ts`）、启动器与 `.rdp` 生成（`main/remoteDesktop/rdpLauncher.ts`）、连接选项存储（`rdpOptions.ts`）、`rd/*` IPC 面（main 常量 → preload → shared `IPlatformService` → renderer 桥四处同步）、设备卡连接入口与置灰判据（`deviceAvailability.ts`）、会话列表（`RemoteDesktopSessionList.tsx`）、被控端设置面（共享开关 + 系统 RDP 状态与指引）、i18n 三份。
- [x] 自研媒体管线删除：Rust `rd_media` 与其测试、Cargo features 收窄、helper 媒体泵、main/host 会话协调器、server 媒体票据与三条路由、`shared/src/remote-desktop.ts`、UI 会话面与其测试、4 份旧 spec/plan。CUA 的 `raw_input`/`clipboard` 与 LAN 发现/配对/kill-switch 未动。
- [x] 本地自动检查（2026-10-15 实测）：`pnpm typecheck` 通过；`pnpm lint` 0 error（84 warning，均为既有）；`pnpm architecture:check --changed` 0 违规；shared 57 / services 320（8 skip）/ server 12 / desktop 57 / ui 远程桌面 18 全部通过；desktop main 单独编译无未解析模块（该工程不在 `pnpm typecheck` 图内，需单独扫）。
- [x] 消息契约守卫：`tests/remote-desktop-host-messages.test.mts` 比对 host/main 两份镜像并禁止已删消息族复活（旧夹具随 `remote-desktop.test.mts` 一并删除，此处重建）。
- [ ] 实机验收：见上「验收场景」，未执行（需两台 Windows，含一台 Pro、一台家庭版）。
- [ ] Rust 侧编译与测试：改动机器无 VS/Windows SDK，`cargo` 无法链接依赖构建脚本，未本地验证；待 CI 的 `cargo test --manifest-path crates/mode-cua-ax/Cargo.toml` 覆盖 features 收窄后的编译。
- P2 未开始（见「分期」）。
