# Spec：P2 命名统一（ZCode / ZCODIUM → Mode）

前置：`p0-user-facing-naming.md`（P0：bin / deep link / i18n / README）与
`p1a-external-env-renames.md`（P1a：对外环境变量 `ZCODE_` → `ZCODIUM_`）已完成。
本 spec 是 P1a 结尾写明的「P2：转独立维护时处理」那一步，并把 `ZCODIUM_` 再推进到
`MODE_`、把第一方包名与产物名统一到 Mode。

## 范围

| 面 | 现在 | 改成 | 兼容策略 |
| --- | --- | --- | --- |
| 对外环境变量 | `ZCODIUM_*`（旧名 `ZCODE_*`） | `MODE_*` | 三段链：读 `MODE_` → `ZCODIUM_` → `ZCODE_`；写全名 |
| 内部进程间变量 | `ZCODE_*`（约 330 个） | `MODE_*` | 无（进程内自洽） |
| 包名 | `@zcode/*`（29 个包） | `@mode/*` | 无（同一提交原子切换） |
| 目录名 | `apps/zcode-cli`、`packages/zcode-*`、`scripts/zcode-distribution`、`src/zcode-protocol*`、`src/zcode-{agent,session}` | `mode-*` | 无 |
| 产物名 | `dist/zcode.cjs`、`zcodium-<版本>.tar.gz` | `dist/mode.cjs`、`mode-<版本>.tar.gz` | 无（安装器与文档同步） |
| 命令名 | `zcodium` | `mode` | 无 alias（P0 已定过同一口径） |
| 用户数据根 | `{base}/.zcodium` | `{base}/.mode` | 多代迁移：`.mode` ← `.zcodium` ← `.zcode`，迁移失败保留旧路径 |
| Electron userData | `ZCodium` | `Mode` | 首次启动迁移旧目录，失败则继续用旧目录 |
| 插件市场 id | `zcode-plugins-official` | `mode-plugins-official` | 落盘数据迁移（设置 / 安装记录 / cache 目录） |
| 插件清单目录 | `.zcode-plugin/plugin.json` | `.mode-plugin/plugin.json` | 新名为主，**继续读旧路径**，第三方插件不作废 |
| 忽略文件 | `.zcodeignore` | `.modeignore` | 读新名，兼容旧名 |
| 项目级配置目录 | `.zcode/` | `.mode/` | 读新名，兼容旧名 |
| 协议名 | `ZCode Protocol` | `Mode Protocol` | 服务端**同时接受**旧名（SSH 远端旧 agent 混布） |
| LAN cookie | `zcode_lite_token` | `mode_lite_token` | 读旧 cookie 兜底 |
| MCP meta 前缀 | `com.zcode/*` | `com.mode/*` | 读旧 key 兜底 + 迁移已存认证 |
| deep link | `zcode://` | `mode://` | 注册新 scheme，并处理旧 scheme 链接；不注册旧 scheme（避免与宿主 ZCode 争抢） |
| 对外标识 | `WECOM_QR_SOURCE`、HTTP `appName` | `mode` / `Mode` | 企业微信侧实机验证，平台拒绝则回退 |
| 版权主体 | `ZCodium contributors` | `Mode` | 只改第一方 LICENSE，上游署名不动 |

## 必须保留（不得改动）

- `LICENSE-APACHE` 与 `scripts/license-texts/Apache-2.0.txt` 的 Z.AI / 占位符署名（Apache-2.0 §4）。
- `NOTICE.md` / `NOTICE.zh-CN.md` 的衍生作品段落（`zai-org/ZCode` 链接、Apache-2.0 说明、"版权与署名声明原样保留"），它是仓库级 §4(b) 修改声明的法律机制本体。
- `THIRD-PARTY-NOTICES.md` 与 `third-party/**` 的全部第三方原文与哈希台账。
- `packages/ui/src/components/ai-elements/*` 头部 Vercel 版权与上游修改标注。
- `.github/workflows/upstream-audit.yml` 里的真实远端仓库名 `ZCodium-project/ZCodium`（功能依赖，非品牌）。
- `.zcode-plugin` / `.claude-plugin` / `.codex-plugin` 的**读取兼容**（旧路径仍可被发现）。

## 验收

1. `pnpm typecheck`、`pnpm lint`、services / desktop / ui 测试、`architecture:check`、`knip` 全绿。
2. dev 实例：旧数据目录自动迁移后设置 / 机器人 / 凭证仍在；插件商店两段正常；已装插件仍启用。
3. 本地打一次 Windows 安装包：产物名、图标、安装器文案均为 Mode；改名后 CLI 产物可安装可运行。
4. 保留项逐条核对未被改动（`git diff` 检查上述文件）。
5. 第三方台账：`packages/desktop/package.json` 等台账输入变更后，在 CI（Linux）用
   `licenses-notices` workflow_dispatch 重生成并入库。
