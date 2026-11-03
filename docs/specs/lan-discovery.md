# 局域网设备发现（广播 + Tailscale 单播 + 单卡合并）

## 产品规则

- **双通道探测，同一窗口收集**（`packages/desktop/src/main/desktopLanDiscovery.ts`）：
  1. 广播探测：向全局/定向广播地址发 `LAN_ACCESS_PROBE`，覆盖同一二层网段。
  2. Tailscale 单播探测：读 `tailscale status --json` 获取 tailnet 内全部 `100.x` IP，
     逐个单播同一份探针到 `LAN_ACCESS_DISCOVERY_PORT`——tailnet 不透广播，单播补上。
- **发现结果 = 可连接的对端，不含本机**：应答地址命中本机网卡地址集（含 127.0.0.1）
  一律在源头过滤——「自己两张卡」是 Tailscale /32 网卡把广播地址算成本机 IP 所致，
  属本机信息，归「设置 → 局域网访问」页展示（该页监听提示显示**全部**本机地址，
  含 Tailscale）。`includeSelf` 选项仅供测试/诊断；本机自连测试改用手动填地址。
- **单卡合并**：同一 `serverId + port` 的多地址（局域网 + tailnet）合并为一张卡，
  `mergeLanDiscoveredPeers` 为唯一纯函数入口：
  - 主地址优先物理局域网（非 `100.64.0.0/10`），tailnet 地址退为 `extraHosts`；
  - 卡片副行展示 `主地址 · 副地址…`（均带端口），点击填入主地址；
    手动填写副地址时该卡仍算选中（命中 `host ∪ extraHosts`）。
  - 端口不同（同安装的异常双实例）不合并，保持分卡。
- **探测口径不变**：只向「广播目标 + 自己 tailnet 状态里的 IP」发探测，不扫网段、
  不发周期广播；应答不含令牌。两通道结果统一走去重与合并。
- **降级**：`tailscale` CLI 缺席/超时（≤800ms）/JSON 解析失败 → 静默跳过单播，
  行为退化为纯广播；总窗口维持 1.5s 量级。
- 可测性：`discoverLanPeers` 支持注入 `tailscaleStatusRunner` 与 `includeSelf`；
  合并逻辑为纯函数单测。

## 验收场景

1. 解析 `tailscale status --json` 样例：Self+Peer 的 IPv4 去重、滤 v6。
2. 注入 runner 返回 `["127.0.0.1"]` 且本地 45879 有应答器：
   `includeSelf: true` 时能发现该应答设备（RED：未实现单播时为空）；
   默认调用时该本机应答被过滤（不出现「自己」）。
3. `mergeLanDiscoveredPeers`：同 serverId+port 的 LAN + tailnet 两地址 → 一张卡、
   主地址为 LAN 侧、`extraHosts` 含 tailnet 地址；端口不同 → 分卡。
4. runner 缺席/抛错：只走广播，不抛错。
5. 实机：装有 Tailscale 的两台设备互扫，非本机 tailnet 设备出现在列表且为单卡；
   发现列表不出现本机；本机地址在「设置 → 局域网访问」监听提示里完整展示。

## 不做

- ping 扫描 `100.64.0.0/10`；Tailscale API / auth key 集成。
- 按可达性自动选路（主地址规则覆盖）。
- 发现列表里的本机徽标（本机不进该列表）。

---

## Rust 迁移（2026-11-03，纯逻辑片）

> crate：`crates/lan-discovery`。真源仍为
> `packages/server/src/remote/lanDiscovery.ts`；本片只迁可注入的纯计算。

### 已迁

| Rust 入口 | TS 对应 | 说明 |
| --- | --- | --- |
| `parse_tailscale_status_ips` | `parseTailscaleStatusIps` | Self+Peer 的 IPv4 去重、滤 IPv6、离线 peer 保留、解析失败返回空 |
| `merge_lan_discovered_peers` | `mergeLanDiscoveredPeers` | 同 `serverId|port` 合并、主地址优先物理局域网、`extraHosts`、端口不同分卡、同 host 后者胜 |
| `compute_broadcast_address` | `computeBroadcastAddress` | 地址与掩码推导定向广播 |
| `collect_broadcast_targets` | `collectBroadcastTargets` | 固定 `255.255.255.255` + 非 internal IPv4 网卡的定向广播，去重 |
| `is_ipv4_address` / `is_tailscale_range_address` | 同名 helper | 严格四段 IPv4；`100.64.0.0/10` |

### 未迁（宿主装配层）

- UDP socket 的 bind/send、1.5s 收集窗口、本机地址过滤、应答去重键与最终排序。
- `tailscale` CLI 候选路径、800ms 超时与 runner 降级；由宿主注入 `tailscaleStatusRunner`。
- 网卡枚举（`networkInterfaces`）与公告解析（`parseLanAnnouncement`，在 shared 契约层）。
- 发现结果最终按 `name ?? serverId` 排序。

### 已知差异

| 差异 | 说明 |
| --- | --- |
| 字符串排序 | TS 用 `localeCompare`；Rust 用字节序。IP/ASCII serverId 一致，非 ASCII 名称可能不同。 |
| 结构体字段 | Rust `LanDiscoveredPeer` 只保留合并所需字段；`magic`/`protocolVersion` 由公告解析层校验。 |
| 端口类型 | Rust 用 `u16`；TS 在公告 schema 已限制 1..=65535。 |

### 验收

- `cargo test -p lan-discovery`：8 条用例覆盖 Tailscale 解析/降级、单卡合并（LAN 优先、
  tailnet 副地址、端口分卡、同 host 覆盖、排序）、IPv4 严格校验、CGNAT 边界、
  定向广播与目标去重。
- UDP 端到端与实机双机验收未开始；Node 实现仍是 oracle。
