# Tauri 图标与 Windows 安装器素材

| 文件               | 用途                                                         |
| ------------------ | ------------------------------------------------------------ |
| `icon.ico`         | Windows 应用图标；同时作为 NSIS 安装器/卸载器图标            |
| `icon.png`         | 1024x1024 应用图标（源）                                     |
| `icon.icns`        | macOS 应用图标                                               |
| `nsis-sidebar.bmp` | NSIS 欢迎/完成页左侧图，164x314、24bpp BMP，由 `icon.png` 生成 |

`nsis-sidebar.bmp` 只在 Windows 安装器里使用（见 `tauri.windows.conf.json` 的
`bundle.windows.nsis.sidebarImage`）。重新生成时保持 164x314 与 24bpp BMP：
NSIS 的 MUI2 侧栏位固定为该尺寸，PNG 或其它尺寸不会被正确显示。图面使用与产品
深色主题一致的纵向渐变 + 居中图标 + `Mode` 字样，避免 NSIS 的默认通用图标。
