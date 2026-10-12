# fnOS（飞牛）原生应用打包骨架

本目录是 fpk 的静态骨架，由 `scripts/build-fnos-fpk.mjs` 组装进安装包：

- `manifest.template`：飞牛应用元数据（`__MODE_VERSION__` / `__MODE_PORT__` 由构建脚本替换）。
- `config/privilege`：以专用应用用户（package）运行。
- `config/resource`：不声明共享目录或系统集成。
- `cmd/`：生命周期脚本。`main` 是进程唯一所有者（start/stop/status + PID 文件）；`lib.sh` 是回调与 main 共享的令牌/权限函数；`install_*`、`upgrade_*`、`uninstall_*`、`config_*` 遵循飞牛契约（0=成功/运行中，1=失败，3=未运行）。
- `ui/config.template`：飞牛桌面入口，指向 `http://<nas>:<port>/?token=__MODE_TOKEN__`；令牌在安装/升级/配置回调与每次 start 时由 `cmd/lib.sh` 写入。
- `build-lib.ts`、`verify.ts`：构建与验收代码（Node 24 直接执行 TS），仅在打包机上运行，不属于 fpk 载荷。
- `icons/`、`ui/images/`：由 `packages/desktop/build/icon.png`（1024×1024）离线缩放而来的 64/256 图标，修改品牌图标后需重新生成。

契约细节（状态所有者、事件顺序、验收场景）见 `docs/specs/fnos-fpk-packaging.md`。
