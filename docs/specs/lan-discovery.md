# 局域网设备发现（广播 + Tailscale 单播）

## 产品规则

- **双通道发现，同一窗口收集**（`packages/desktop/src/main/desktopLanDiscovery.ts`）：
  1. 广播探测（现状保留）：向全局/定向广播地址发 `LAN_ACCESS_PROBE`，覆盖同一二层网段。
  2. **Tailscale 单播探测（新增）**：读 `tailscale status --json` 获取 tailnet 内全部
     `100.x` IP，把同一份 `LAN_ACCESS_PROBE` **逐个单播**到 `LAN_ACCESS_DISCOVERY_PORT`。
     Tailscale 虚拟网段不透广播，广播发现不了 `100.x` 设备——单播补上这一段，
     应答回包的 `rinfo.address` 即对端 tailnet IP，直接作为可填地址。
- **口径不变**：只对「自己 tailnet 状态里已知的 IP」发单播，不做网段/端口扫描、
  不发周期广播；探测只问「你是谁」，应答不含令牌（见 lanAccess 契约）。
- **降级**：`tailscale` CLI 不在 PATH 与已知安装路径、超时（≤800ms）、JSON 解析失败
  ——一律静默跳过单播步骤，行为与纯广播完全一致；总收集窗口仍为 1.5s 量级。
- 两通道结果并入同一 `Map`，按 `serverId@host:port` 去重后按名称排序（现状不变）。
- 可测性：`discoverLanPeers` 接受可注入的 `tailscaleStatusRunner`（默认实现跑真 CLI），
  测试注入 `127.0.0.1` + 本地 UDP 应答器即可离线验证单播路径。

## 验收场景

1. 解析 `tailscale status --json` 样例：拿到 Self 与全部 Peer 的 IPv4，去重。
2. 注入 runner 返回 `["127.0.0.1"]` 且本地 45879 有应答器时，`discoverLanPeers`
   返回该应答设备（host=127.0.0.1）——RED：当前实现只广播，单播不存在时为空。
3. runner 缺席/抛错/超时：只走广播，不抛错。
4. 实机：装有 Tailscale 的两台设备互扫，无需手填 `100.x` 地址即可出现在发现列表。

## 不做

- 主动 ping 扫描 `100.64.0.0/10`（有 CLI 就不扫网段）。
- Tailscale API / auth key 集成（只读本地 CLI 状态）。
