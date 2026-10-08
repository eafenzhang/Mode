# 局域网配对设备管理（设置页两组 + 向导撤显示）

## 产品规则

### 向导（新建连接 → 局域网）

- 不再显示「✓ 已配对：xxx」状态行（`remote.lan.paired` 引用与文案一并清理）。
- 配对态行为不变：已配对时不显示配对码输入、直接可连接；未配对时照常输入配对码。
- 「发现的设备」列表、地址/端口输入不变。

### 设置 → 局域网访问：两组并列展示

1. **「我配对的对端」（客户端视角，本机作为客户端配对出去的设备）**
   - 本机「允许局域网访问」开关关闭时也显示（它不依赖本机监听）。
   - 每行：名称、地址（`host:port`）、**对端提供的工作区目录**（只读，按需向对端 `/api/server-info?token=` 拉取；离线/令牌失效显示「无法获取」）、**删除**。
   - 删除 = 清除该 serverId 的长期令牌与配对元数据；此后向导连接该设备需重新输配对码。
   - 空态：提示尚未配对任何对端。
2. **「配对我的设备」（服务端视角，现有组）**
   - 保留：label、最近使用时间、逐个删除、重置全部令牌。
   - **label = 配对方设备的主机名**：`pairLanPeer` 在 host 执行，未显式传 label 时用
     `node:os` 的 `hostname()` 兜底；向导不再传 `navigator.platform`（Windows 上它是
     `"Win32"`，就是设置页显示 win32 的根因）。显式传 label 的路径保留（验证脚本用）。
     存量 "win32" 记录服务端无法回填名称，重新配对一次后更新为主机名。
   - 每行新增**最近在本机打开的目录**；无记录显示「尚无」（老数据向后兼容）。

### 数据与状态所有权

- 客户端侧配对元数据 `lan:peer:<serverId>:meta`（`{host, port, name?}`）：在 `pairLanPeer` 换到令牌的同一处写入，与 `lan:peer:<serverId>:token` 同生命周期。
- 枚举与远端目录获取走平台接口（renderer 不直接枚举凭据）：
  - `getLanPairedPeers(): Promise<LanPairedPeer[]>`
  - `getLanPeerWorkspaces(serverId): Promise<LanPairedPeerWorkspaces>`（host 用已存令牌请求对端 server-info）
  - `removeLanPairedPeer(serverId): Promise<void>`
- 服务端侧 `LanAccessClientRecord.lastWorkspacePath`：**唯一所有者 = lanAccessServer**（读写都在 host 进程，沿用 clients 记录的持久化路径）。
  - 更新时机：该客户端所持连接（`/ws/host` 升级请求 cookie 解析出 clientId）上出现**携带 `workspacePath` 的 RPC**（对 `registerChannel` 做通用录制，call/listen 均计；对端目录浏览的 `readdir`/`resolvePath` 用 `path` 字段，不会误录）。低频、直接持久化。
  - 事件顺序：升级（cookie → clientId）→ 客户端打开工作区 → 任务类 RPC 携带 `workspacePath` → 录制回调 → clients 记录更新 → 设置页下次拉取可见。
- i18n：zh-CN / en-US / fa 三语同步。

## 验收场景

1. 向导 LAN 步骤在目标已配对时：无「已配对」行、无配对码输入，直接可连；未配对时配对码输入照常。
2. 设置页两组同屏；本机 LAN 开关关闭时「我配对的对端」仍显示；对端离线时其工作区目录显示「无法获取」。
3. 删除「我配对的对端」某行后，向导对该设备重新要求配对码。
4. 服务端组：新配对设备显示「尚无」；对端在本机打开某目录后，该行显示此目录（server 包测试用真实 WS 打一条携带 `workspacePath` 的 RPC 断言录制回调）。
5. 配对成功后服务端组 label 显示配对方主机名（host 不传 label 时 `pairLanPeer` 默认
   `hostname()`；desktop 夹具断言请求带上 label 而非 undefined）。
6. `pnpm --filter @mode/services test`、`pnpm --filter @mode/server test`、desktop 测试、`pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed` 全绿。

## 不做

- 目录项点击发起连接（YAGNI，未被要求）。
- 改名前版本的兼容（用户已拍板不做，见记忆 lan-no-pre-rename-compat）。
