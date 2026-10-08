# Mode macOS DMG 安装体验

## 规则与边界

- `electron-builder.config.js` 的 `dmg` 节点是 DMG 窗口布局、背景和卷内文件的唯一所有者；`build/dmg_background.png`（620x560）与 `build/dmg_background@2x.png`（1240x1120）是背景事实源，窗口尺寸由 1x 背景图决定。Finder 窗口外框比图标视图可见区高约 70px（工具栏），所有元素必须排在顶部可见区内。
- 安装包没有 ZCode 官方签名，首次打开必被 Gatekeeper 拦截；面向用户的所有文案统一用"没有 ZCode 官方签名"，不展开 Apple 公证/代码签名等平台细节。DMG 必须同时提供背景说明和可复制的纯文本说明文件：`build/dmg/解除拦截.txt`（中文名）与 `build/dmg/Unblock.txt`（英文名，不带语言后缀），内容包含 `sudo /usr/bin/xattr -rd com.apple.quarantine "/Applications/Mode.app" && open -a "Mode"`（解除后自动启动、命令立即退出，不挂起终端）、先拖拽后执行的时机说明、密码不回显提示与替代打开方式。
- 解除命令一律使用 `/usr/bin/xattr` 绝对路径：用户 PATH 中可能存在同名工具（如 Python 的 xattr 包），裸 `xattr` 会报 `option -r not recognized`。
- 背景上的两个箭头分别指向两个说明文件的落点（contents 坐标 x=170 / x=450，y=370）；app 图标在 (150, 120)，Applications 链接在 (470, 120)，中间箭头表示拖拽方向。布局最低内容（说明文件标签）不超过可见区下沿，保证 Finder 打开时不出现滚动裁切。
- 说明文件通过绝对路径写入 `dmg.contents`，不依赖 dmgbuild 的工作目录；不隐藏扩展名。
- 应用图标资源（icns/ico/png）必须带透明背景；macOS icns 使用 824/1024 内容区居中，避免 Dock/启动台显示白色方块。图标素材的事实源是 `build/icons/1024x1024.png`。
- `public/logo/icons/*` 与 `public/icon_512@2x.png` 是同一项目标记的对外副本（README、更新对话框与网站静态资源），必须与 `build/icons/*` 同源同内容，并保持透明背景。

## 验收

- 重新打包 macOS DMG 后，挂载检查：窗口为 620x560、背景显示 Mode（不再出现 ZCODE）、可见两个说明文件与箭头、`.app` 可拖入 Applications，且内容不需要滚动即可完整看到。
- 打开两个说明文件，确认使用 `/usr/bin/xattr` 绝对路径、说明先拖拽再执行、密码输入不回显、解除后自动启动且命令立即退出（不挂起），且命令可整行复制。
- 从 DMG 安装后的 `.app` 在 Dock 与启动台中显示黑底圆角图标（不再出现白色方块）。
- `public/logo/icons/*` 与 `public/icon_512@2x.png` 的四角 alpha 为 0，与 `build/icons/*` 内容一致；更新对话框（`UpdateStatusDialog.tsx`）和 README 不再出现白色方块。
- `pnpm typecheck`、`pnpm lint` 通过；DMG 产物名保持 `Mode-<version>-mac-<arch>.dmg`。
