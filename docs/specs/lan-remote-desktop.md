# 局域网远程桌面（历史存根 · 现役唯一路径 = 应用内自研）

状态：**本文已作废，只作历史索引**（2026-10-15 二次变更）。

现役唯一权威：**[in-app-remote-desktop.md](./in-app-remote-desktop.md)**（应用内自研媒体面）。
本文原有的产品规则、状态所有者、事件顺序、坐标/光标规则与媒体面设计**已并入该 spec 并被其取代**；
二代实现同在该 spec 指定的 git 基线（`54aeb907^`）里恢复。

变更轨迹（供追溯，不要据此实现）：

1. 2026-10-12：本文 + [lan-remote-desktop-native-media.md](./lan-remote-desktop-native-media.md)
   定义自研两代媒体面（WebRTC → 原生 DXGI/MF 直替）；
2. 2026-10-15 上午（`54aeb907`，发版 v0.0.42）：整体删除自研链路，改为启动系统 RDP（mstsc），本文收敛为存根；
3. 2026-10-15 下午：用户决定**恢复自研并取代系统 RDP**——远程桌面只保留应用内一种形态，
   mstsc 路径与其 `systemRdp` 能力位、连接选项、会话列表一并移除（同一次 `revert 54aeb907` 交付）。
   取代理由：应用内画面、家庭版可被控（RDP 主机需 Pro/Enterprise）、应用级同意/授权/指示。

仍然有效且未变更：LAN 发现与配对（[lan-discovery.md](./lan-discovery.md)、
[lan-remote-attach.md](./lan-remote-attach.md)、[lan-paired-devices.md](./lan-paired-devices.md)）、
设备页与侧边栏入口、被控端 kill-switch 语义（[computer-use-enablement.md](./computer-use-enablement.md)）。
