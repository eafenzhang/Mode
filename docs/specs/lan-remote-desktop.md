# 局域网远程桌面（已被取代）

状态：**已被 [remote-desktop-system-rdp.md](./remote-desktop-system-rdp.md) 取代**（2026-10-15）。

本文原为自研媒体面方案的权威（WebRTC → 原生 DXGI/MF 直替两代），其内容已全部作废：

- 自研采集/编码/传输/解码/输入注入链路已整体删除（Rust `rd_media`、helper 媒体泵、媒体中继、双票据、媒体 WS、信令 v2）。
- 应用内会话面（画布 / 操作栏 / 状态条）、画质档位、自适应码率、显示器选择、坐标映射与键盘决策纯函数一并删除。
- 产品规则中的同意窗、进行中指示条、逐设备授权、被控中不可发起、输入共享（无接管互斥）作废——原因见新 spec 的「让渡的」一节。

仍然有效且未变更的：LAN 发现与配对（[lan-discovery.md](./lan-discovery.md)、[lan-remote-attach.md](./lan-remote-attach.md)、[lan-paired-devices.md](./lan-paired-devices.md)）、设备页与侧边栏入口、被控端 kill-switch 语义（[computer-use-enablement.md](./computer-use-enablement.md)）。

产品规则、状态所有者、接口、验收场景一律以 [remote-desktop-system-rdp.md](./remote-desktop-system-rdp.md) 为准。
