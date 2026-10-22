# 局域网配对设备管理（设置页两组 + 向导仅已配对设备）

## 产品规则

### 向导（新建连接 → 局域网）

2026-10-22 调整：新建连接向导重新提供「局域网」方式，但只负责**选择已配对设备**，
配对仍旧在「远程控制」页完成。

- 数据源唯一：`getLanPairedPeers()`；不调用 `discoverLanPeers()`，不显示发现的未配对设备。
- 不提供地址/端口输入与配对码输入，向导不写 `lan:peer:*`，不在本页产生新的配对记录。
- 选中已配对设备后直接可连接；连接目标必须带上该记录的完整 `endpoints`（公网优先、
  LAN 回退），不能只投影 `host/port`，否则公网-only 设备会连到不可达的 LAN 地址。
- 没有已配对设备时显示空态，提示用户先到「远程控制」页配对；不提供跳转按钮，
  避免向导与设置页互相导航。
- 未选择任何设备时「下一步」给出「请选择已配对设备」校验提示，不回落到手动地址校验。

### 设置 → 远程控制：被控设备单列表 + 我配对的对端

被控面（原「配对我的设备」组）与远程桌面被控面**合并为一张卡片内的单列表**（2026-10-13 拍板；原独立的「已授权设备」列表并入）：

- 卡片内自上而下：允许局域网访问开关（描述行展示监听地址/端口）、允许远程控制开关、配对码、**已配对设备**统一列表、重置全部令牌。
- **「已配对设备」列表行 = 配对记录本身**：label、最近使用时间、最近在本机打开的目录（无记录显示「尚无」）、**远程桌面授权开关**（checked = 该记录 `remoteDesktopAuthorized === true`，与同意窗「记住此设备」写同一字段）、**移除**（删令牌与元数据，并同时撤销远程桌面授权）。
- 列表与配对记录同寿命：**本机监听未开启时也从凭据读取展示**（`readDisabledLanAccessState`），不依赖 lanAccessServer 在跑；此时配对码与重置为不可用（需要服务端）。
- **label = 配对方设备的主机名**：`pairLanPeer` 在 host 执行，未显式传 label 时用
  `node:os` 的 `hostname()` 兜底；向导不再传 `navigator.platform`（Windows 上它是
  `"Win32"`，就是设置页显示 win32 的根因）。显式传 label 的路径保留（验证脚本用）。
  存量 "win32" 记录服务端无法回填名称，重新配对一次后更新为主机名。

1. **「我配对的对端」（客户端视角，本机作为客户端配对出去的设备）**
   - 本机「允许局域网访问」开关关闭时也显示（它不依赖本机监听）。
   - 每行：名称、地址（`host:port`）、**对端提供的工作区目录**（只读，按需向对端 `/api/server-info?token=` 拉取；离线/令牌失效显示「无法获取」）、**删除**。
   - 删除 = 清除该 serverId 的长期令牌与配对元数据；此后向导连接该设备需重新输配对码。
   - 空态：提示尚未配对任何对端。

### 契约：`LanAccessClientRecord` 与状态读取

- `LanAccessClientRecord` 显式声明 `remoteDesktopAuthorized?: boolean`（缺键 = 未授权；旧数据向后兼容）。
  **shared 的 strict schema 必须同步声明该字段**——2026-10-13 修复：host 回帖携带该字段而
  schema 缺声明时，main 的 `hostResponseMessageSchema` 整条拒收 `lan-access-state-result`，
  UI 表现为 `mode:get-lan-access-state` 8s 超时。
- `LanAccessState` 读取语义：`enabled=false` 时 `clients` 仍返回持久化配对记录（读凭据），
  `port/addresses/pairCode` 为空；只有服务端动作（生成配对码/移除/重置）需要监听在跑。
- 客户端侧配对元数据 `lan:peer:<serverId>:meta`：v2 为
  `{endpoints: [{scheme, host, port, kind}], name?, pairedAt?}`，端点公网优先、LAN 回退
  兜底；v1 `{host, port, name?}` 读取时迁移为单个 LAN 端点。在 `pairLanPeer` 换到令牌的
  同一处写入，与 `lan:peer:<serverId>:token` 同生命周期。公网端点规则见
  [lan-remote-public-endpoints.md](./lan-remote-public-endpoints.md)。
- 枚举与远端目录获取走平台接口（renderer 不直接枚举凭据）：
  - `getLanPairedPeers(): Promise<LanPairedPeer[]>`
  - `getLanPeerWorkspaces(serverId): Promise<LanPairedPeerWorkspaces>`（host 用已存令牌请求对端 server-info）
  - `removeLanPairedPeer(serverId): Promise<void>`
- 服务端侧 `LanAccessClientRecord.lastWorkspacePath`：**唯一所有者 = lanAccessServer**（读写都在 host 进程，沿用 clients 记录的持久化路径）。
  - 更新时机：该客户端所持连接（`/ws/host` 升级请求 cookie 解析出 clientId）上出现**携带 `workspacePath` 的 RPC**（对 `registerChannel` 做通用录制，call/listen 均计；对端目录浏览的 `readdir`/`resolvePath` 用 `path` 字段，不会误录）。低频、直接持久化。
  - 事件顺序：升级（cookie → clientId）→ 客户端打开工作区 → 任务类 RPC 携带 `workspacePath` → 录制回调 → clients 记录更新 → 设置页下次拉取可见。
- **对端暴露的工作区清单只含用户选择的路径**（`collectLanAccessWorkspaces` → server-info）：
  本机默认对话工作区（数据根下 `workspace/default`，非项目会话目录）属内部目录，
  不外发——由 `isInternalConversationWorkspacePath`（shared，纯函数，win32 大小写
  不敏感、POSIX 精确、收尾分隔符归一）在收集入口过滤；最近项目与用户打开过的
  会话目录照常暴露。
- i18n：zh-CN / en-US / fa 三语同步。

## 验收场景

1. 向导 LAN 步骤只列出已配对设备：可选中并连接；没有地址/端口、配对码、发现列表与重新搜索。
2. 没有已配对设备时显示空态提示去「远程控制」配对；未选择设备时下一步提示「请选择已配对设备」。
3. 设置页两组同屏；本机 LAN 开关关闭时「我配对的对端」仍显示；对端离线时其工作区目录显示「无法获取」。
4. 删除「我配对的对端」某行后，该设备从向导 LAN 列表消失，且向导内无法重新配对。
5. 服务端组：新配对设备显示「尚无」；对端在本机打开某目录后，该行显示此目录（server 包测试用真实 WS 打一条携带 `workspacePath` 的 RPC 断言录制回调）。
6. 配对成功后服务端组 label 显示配对方主机名（host 不传 label 时 `pairLanPeer` 默认
   `hostname()`；desktop 夹具断言请求带上 label 而非 undefined）。
7. 对端 lastWorkspaceSession 含默认对话工作区时，server-info.workspaces **不含**
   该目录；用户打开过的项目/会话目录照常出现（shared 单测覆盖过滤判定各分支）。
8. `pnpm --filter @mode/shared test`、`pnpm --filter @mode/services test`、
   `pnpm --filter @mode/server test`、desktop 测试、`pnpm typecheck`、`pnpm lint`、
   `pnpm architecture:check --changed` 全绿（shared 测试已入 CI 流水线）。

## 不做

- 目录项点击发起连接（YAGNI，未被要求）。
- 改名前版本的兼容（用户已拍板不做，见记忆 lan-no-pre-rename-compat）。
