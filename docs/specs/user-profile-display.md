# 左下角用户资料与昵称

状态：2026-10-21 实现。关联：
[mode-cloud-account-directory.md](./mode-cloud-account-directory.md)（Mode Cloud 账号与绑定）、
[p0-user-facing-naming.md](./p0-user-facing-naming.md)（用户可见命名）。

## 目标

1. 首页与设置页左下角的账号区展示同一份用户资料，不再出现一处是品牌名、另一处是微信 id 的分裂。
2. 用户登录后，没有头像时显示统一的默认用户图标，而不是用显示名首字母拼头像。
3. Mode 账号弹窗支持设置本地昵称，用于左下角账号区与设置页展示。

## 产品规则

1. **账号身份只认两种来源**：renderer 的 OAuth 登录用户（`mode store.user`）或
   Mode Cloud 已绑定账号（`cloudAccount.status === "bound"`）。
2. **本地微信 Bot 绑定不是用户资料**：`boundWeixinName`、Bot 的 `providerUserId`（如
   `o9cg...` 的微信 openid）不得作为左下角显示名。它只代表本机绑定了哪个微信 Bot。
3. **显示名优先级**（仅在前两条身份来源之一成立时生效）：
   本地昵称 → OAuth `displayName` → OAuth `username` → Cloud `account.displayName` → `Mode`。
4. **头像规则**：
   - 有 `avatarUrl`：显示头像图片；
   - 已登录但没有头像：显示默认 `UserRound` 用户图标；
   - 未登录：显示 Mode 品牌图标。
5. **昵称属于本机偏好**：存入 `AppSettings.userNickname`，最长 32 字符，首尾空白会被裁剪；
   保存空串表示删除覆盖，回退到账号名。昵称不修改 OAuth / 云端账号资料，也不随云目录同步。
6. **昵称编辑入口**：Mode 账号弹窗（左下角头像菜单 → Mode 账号）。未登录时输入框不展示可写状态，
   只提示先登录 / 绑定账号；登录状态在同一弹窗内完成绑定后即时可编辑。
7. **两端一致**：首页侧栏与设置页复用 `WorkspaceSidebarFooter` / `sidebarProfile` 解析；
   Cloud 账号状态从本机 base services 读取（账号是 app-global、local-only 能力，不能被远端 workspace
   service 集合覆盖）。
8. **应用版本号**：左下角账号区在显示名/品牌名旁显示 `v<MODE_VERSION>`，来源是构建期
   `@mode/shared` 的 `MODE_VERSION`，只读、不新增持久化状态。版本号始终展示（登录与否都显示），
   不受昵称、Cloud 绑定或远端 workspace 影响；未注入构建常量时回退 `0.0.0-dev`，不隐藏。

## 状态所有者

| 状态           | 所有者                                               | 说明                                 |
| -------------- | ---------------------------------------------------- | ------------------------------------ |
| OAuth 用户资料 | renderer `useModeStore.user`                         | 登录/恢复/退出由 Root OAuth 流程写入 |
| Cloud 账号绑定 | Host `ICloudAccountService`                          | local-only；事件驱动 footer 与弹窗   |
| 本地昵称       | Host `ISettingService`（`AppSettings.userNickname`） | 本机持久化，`useSettings` 共享快照   |
| 头像展示       | `sidebarProfile` 纯函数                              | 输入为上面三者；不新增持久化状态     |

## 验收场景

1. 只绑定了微信 Bot、未登录 Mode Cloud：首页与设置页都显示 `Mode`，不显示 `o9cg...`。
2. Mode Cloud 已绑定但没有 `account.displayName`：两处显示 `Mode` + 默认用户图标；设置昵称后
   两处立即显示昵称。
3. OAuth 用户有头像：两处显示头像图片；无头像：显示默认用户图标，不显示首字母。
4. 昵称填 32 个字符可保存，超过 32 个字符被输入框限制；清空昵称并保存后回退到账号名。
5. 在设置页修改昵称后返回首页，侧栏展示无需重启即可跟随（同一 settingService 快照）。
6. 首页与设置页左下角都显示与当前构建一致的 `v<MODE_VERSION>`；登录、退出、改昵称都不改变该值。
