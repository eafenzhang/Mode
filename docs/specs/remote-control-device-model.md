# 远程控制统一设备模型（Device / Endpoint / Channel / Session）

状态：2026-10-21 拍板并实施。关联：
[remote-control-unified-pairing.md](./remote-control-unified-pairing.md)、
[lan-remote-public-endpoints.md](./lan-remote-public-endpoints.md)、
[in-app-remote-desktop.md](./in-app-remote-desktop.md)、
[remote-workspace-identity.md](./remote-workspace-identity.md)、
[mode-cloud-account-directory.md](./mode-cloud-account-directory.md)、
[legacy-windows-remote-agent.md](./legacy-windows-remote-agent.md)。

## 背景

远程控制能力由五条链路组成：远程工作区（`/ws/host` RPC）、远程桌面（信令 +
媒体 WS）、Mode Agent（MCP HTTP）、云账号设备目录（配对入口）、Web/手机远控
（`web-remote-replayable`）。此前“设备”“配对码”“端点”在每个链路各有一套投影，
导致同名不同义、入口分散、端点能力不对称。本 spec 固定一层**产品模型**，所有 UI
与连接代码都按该模型取设备、端点和通道；底层协议、票据与会话所有者不变。

## 模型

```ts
type RemoteDeviceOrigin = "lan" | "discovered" | "mode-agent" | "cloud";

type RemoteEndpointKind = "lan" | "public";
interface RemoteEndpoint {
  scheme: "http" | "https";
  host: string;
  port: number;
  path?: string; // 中继/反代前缀，如 /d/<tunnelId>
  kind: RemoteEndpointKind;
}

type RemoteChannel = "workspace" | "desktop" | "conversation";

interface RemoteDevice {
  id: string; // serverId = 云 deviceId；Mode Agent 用 MCP serverName
  origin: RemoteDeviceOrigin;
  name: string;
  endpoints: RemoteEndpoint[]; // 公网在前、LAN 在后
  online?: boolean;
  channels: {
    workspace: boolean; // 可 attach RPC（LAN 或公网 HTTPS）
    desktop: boolean; // 能力位 remoteDesktop && remoteDesktopNative
    conversation: boolean; // Mode Agent（MCP）
  };
}
```

- **Device** 是产品面唯一“设备”概念；`RemoteControlHome` 只消费 `RemoteDevice[]`。
- **Endpoint** 是唯一地址事实源；`host`/`port` 只是主端点的兼容投影。
- **Channel** 是设备上可用的能力，不是设备类型；一台 LAN 设备可同时具备
  `workspace` 与 `desktop`，一台 Mode Agent 只有 `conversation`。
- **Session** 仍由既有所有者持有：远程工作区归 `windowRemoteConnectionRegistry`，
  远程桌面归 `RemoteDesktopHostServer` / 主控 controller。本模型只做投影，不新增
  第二份会话状态。

## 产品规则

1. **唯一设备入口**：远程控制页（`RemoteDesktopHome`）是唯一持久设备列表；云账号弹窗
   只承担“登录 + 从云目录添加设备”的临时流程，配对成功后设备回到本页。设备行按
   `channels` 渲染动作：
   - `workspace` → 本页不提供入口（远程工作区从新建连接向导进入）；
   - `desktop` → “进入桌面”；
   - `conversation` → “发起对话”。
   新建连接向导负责 SSH/WSL/Docker，以及**只读选择已配对设备**的 LAN 方式；
   LAN 的发现/配对/管理仍只在本页（[lan-paired-devices.md](./lan-paired-devices.md) §向导），
   向导不得写入配对记录或提供手动地址/配对码。
2. **配对码唯一**：6 位十六进制、5 分钟 TTL、单次消费；同时服务
   `/api/lan/pair` 与 `/api/remote-agent/enroll`（[remote-control-unified-pairing.md](./remote-control-unified-pairing.md)）。
   云端微信登录的 6 位十进制码与 Bot `/bind` 码不叫“配对码”，UI 必须区分文案。
3. **端点统一**：同一 `LanPeerEndpoint[]` 同时供远程工作区与远程桌面使用；顺序固定
   `public` 在前、`lan` 在后。远程工作区与远程桌面都按该顺序探测，传输失败/超时/
   5xx/404/协议不符才尝试下一个端点；401/403 与合法 `server-info` 立即收口。
4. **WSS 规则**：`https` 端点的 HTTP 用 HTTPS，信令/媒体/工作区 WS 用 `wss`；
   任何路径都不允许把公网端点降级成明文 HTTP。
5. **SSH/WSL/Docker**：Tauri Host 复用 `@mode/server/remote` 的 `createRemoteBackend`
   与 `connectRemote`，与 LAN 共用 `windowRemoteConnectionRegistry` 的 logical session /
   attachment 生命周期；连接成功后的服务面与 LAN 一致，不新增第二套远程会话链路。
   remote assets 源优先取 `MODE_REMOTE_ASSET_CDN_BASE_URL`（含旧名回退），其次构建期
   define，最后开发态 `packages/desktop/mock-cdn`；都没有时在发起连接前返回可读错误，
   不进入半截部署。
6. **Web/手机远控**：桌面 Host 的 LAN HTTP 服务挂载 Web 静态资源（`staticRoot`），
   浏览器首次进入用被控端 6 位配对码换取 LAN 令牌（`/api/lan/pair`，响应写
   `mode_lite_token` HttpOnly Cookie），随后复用现有鉴权；`web-remote-replayable`
   语义不变。桌面安装包不再携带 `packages/web/dist`：桌面 renderer 的统一构建同时产出
   `index.html`（Tauri）与 `web.html`（浏览器），安装包只内嵌一份前端；壳启动时把内嵌资源
   解压到 `web-assets/<version>` 并设置 `MODE_WEB_STATIC_ROOT`，LAN 服务以 `web.html` 为入口
   （详见 [unified-frontend-build.md](./unified-frontend-build.md)）。开发态仍可用
   `MODE_WEB_STATIC_ROOT` 覆盖。
7. **迁移**：旧 `lan:peer:*:meta`（只有 `host`/`port`）读取时迁移为单个 LAN 端点；
   旧 `RemoteTarget`（只有 `host`/`port`）按 http + LAN 端点处理，不要求重新配对。

## 状态所有权

| 状态 | 唯一所有者 | 说明 |
| --- | --- | --- |
| 设备列表投影 | `RemoteDesktopHome`（页级瞬态） | 每次进入/刷新重读四类来源并合并；不持久化 |
| 配对记录 / 令牌 / 端点元数据 | Host 凭据服务 | `lan:access:clients`、`lan:peer:<serverId>:token/meta` |
| Mode Agent 设备 | MCP 配置 `mode-agent-*` | 既有事实源 |
| 云设备目录 | Mode Cloud SQLite + UI 页级快照 | 既有事实源；配对后落回 LAN peer |
| 远程工作区会话 | `windowRemoteConnectionRegistry` | 既有 owner |
| 远程桌面会话 | `RemoteDesktopHostServer` / 主控 controller | 既有 owner |
| Web 配对令牌 | LAN 服务（`/api/lan/pair`）→ 浏览器 Cookie | Cookie 只是令牌副本，真相仍在被控端 client 记录 |

## 事件顺序

```text
加载设备：getLanPairedPeers + discoverLanPeers + MCP 配置 (+ 云目录)
  → 端点排序（public → lan）
  → 能力位探测（server-info / MCP status）
  → 合并为 RemoteDevice[]
  → 行动作按 channels 渲染

连接工作区（仅新建连接向导）：已配对设备 → 选第一个可达端点（server-info）
  → attachLanRemoteConnection(scheme/path) → /ws/host → scoped attachment

连接桌面：RemoteDevice → 选第一个可达端点
  → POST /api/remote-desktop/session → 同意 → 双票据
  → 信令 WS + 媒体 WS（scheme 同端点）

Web 首次进入：浏览器打开 / → 输入被控端 6 位码
  → POST /api/lan/pair → 写 mode_lite_token Cookie → 重载 /ws
```

## 失败语义

- 所有端点都不可达 → 设备离线；行内动作给出“对端不可达”，不再尝试第二条链路。
- 401/403 → 令牌失效，提示重新配对；不切换端点。
- 公网端点证书失败/超时 → 尝试 LAN 端点；LAN 也失败才判离线。
- 仅公网端点的设备 → `workspace` 与 `desktop` 都可用；不再被向导过滤。
- SSH/WSL/Docker 连接失败 → 沿用 `connectRemote` 的可读错误；不伪装成 LAN 设备。
- Web 配对码错误/过期 → `/api/lan/pair` 401，页面保留配对码输入并提示重新生成。

## 验收场景

1. 远程控制页是唯一配对/设备管理入口；LAN 行只保留“进入桌面”（能力位满足时），
   不再出现“连接工作区”按钮。远程工作区统一从新建连接向导选已配对 LAN 设备进入。
2. 同一台已配对设备保存 LAN + 公网端点；公网可达时工作区（向导）与桌面（本页）
   都命中公网，公网断开后回退 LAN，`https → wss`，不出现明文降级。
3. 仅公网端点的设备可以在新建连接向导里连接工作区、在远程控制页进入桌面，不被过滤。
4. Tauri Host 下 SSH/WSL/Docker 向导可以建立远程工作区，目录选择、文件、终端、
   会话与 LAN 工作区行为一致。
5. 桌面 LAN 服务 `GET /` 返回 Web 应用；浏览器输入 6 位配对码后建立
   `web-remote-replayable` 连接；错误码提示可重试。
6. 旧配对记录（只有 host/port）不重新配对即可继续使用。
7. `pnpm typecheck`、`pnpm lint`、`pnpm architecture:check --changed` 与相关测试全绿。
