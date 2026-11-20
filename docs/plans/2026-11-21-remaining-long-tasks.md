# 剩余改造长任务清单（2026-11-21）

> 上位：`docs/plans/2026-10-29-full-rust-rewrite.md` §10、
> `docs/plans/2026-11-16-upstream-capability-long-tasks.md`。
> 每个 LT 开工前先补该任务的 spec/细计划；本文件只做 program-level 排序与出口定义。

## 总览

| ID | 长任务 | 依赖 | 出口条件 | 状态 |
| --- | --- | --- | --- | --- |
| **LT-7** | IM SDK 在线升级发布侧 | 无 | Release 挂载 `manifest.json` / `.sig` / `bundle.tar.gz`；配置公钥后客户端可在线激活 | 🟡 工具链完成；签名 secret、客户端公钥、设置页、壳资源注入待做 |
| **LT-8** | W5 TUI 改 ratatui | 无强依赖 | 33 个场景清单逐项过 + 真机交互验收 | ⬜ 未开始 |
| **LT-9** | W5 Web/远程服务端改 axum | W2/W4 稳定 | 契约对拍 + 手机远控验收 | ⬜ 未开始 |
| **LT-10** | W5 插件市场逻辑进壳 | `plugin-host` 资产层 | 目录/安装/卸载走壳，素材仍按资产层 | ⬜ 未开始 |
| **LT-11** | W6 收口 | LT-7 ~ LT-10 | `stdioFrames`/`stdoutGuard` 退场、删 TS 宿主编排与 `host-sidecar` 发运集、插件宿主换嵌入引擎、四仪表终值复测 | ⬜ 未开始 |
| **LT-12** | W3 剩余域 | W1/W2 | provider 写路径/usage 网络凭据、LAN WS/公网中继、CUA PiP/Windows helper host、远程工作区、云账号 | 🟡 部分已迁 |
| **LT-13** | W1 SQLite 迁移门禁 | 无 | 基线改由 Rust 迁移生成 + `--base-ref` 只增校验 | ⬜ 未开始 |
| **LT-14** | LT-1 C4 唯一 PARTIAL 收尾 | 真机 | 20 任务全 PASS；随后删 Node 回退路径 | 🟡 19 PASS / 1 PARTIAL |

## LT-7 本轮交付（2026-11-21）

- **打包器** `scripts/im-sdk-runtime/build.ts`：按 `packages/services/package.json` 的精确版本
  安装生产依赖树 → 确定性 tar.gz + 清单；排除 `.d.ts`/sourcemap，拒绝符号链接与原生二进制。
  真实包：589 文件、1.56 MB，lark 1.64.0 / wecom 1.0.7，入口解析正确。
- **签名** `scripts/im-sdk-runtime/sign.ts`：`generate` / `sign` / `public-key` / `verify`；
  私钥走 `MODE_IM_SDK_RUNTIME_SIGNING_KEY`，不落日志。
- **CI** `.github/workflows/im-sdk-runtime.yml`：`release: published` 或手工触发；secret 未配置
  时自动 dry-run（临时密钥、仅 workflow artifact）；清单稳定地址
  `https://github.com/eafenzhang/Mode/releases/latest/download/im-sdk-runtime-manifest.json`。
- **验证** `pnpm im-sdk-runtime:test` 3/3：打包/清单/双向验签、本地 HTTP → refresh → 激活 →
  加载入口、bundle 篡改拒绝且不切 active；另用真实 SDK 包完成一次安装与两个入口加载。
- **待做**：生成并配置发布密钥（secret + 客户端公钥）；首个 Release 真实上传；设置页版本/
  检查更新；壳资源注入 manifest URL/公钥。

## 建议执行顺序

1. **LT-7 收尾**（配置密钥 + 设置页/壳资源）——把已交付的在线升级真正接通。
2. **LT-8 / LT-9**（可并行，各自先出 spec）——TUI 与 axum 是 W5 剩余的两大块。
3. **LT-10** → **LT-11**（W6 依赖生态面完成）。
4. **LT-12 / LT-13 / LT-14** 按门禁与真机资源穿插；LT-14 只需补一个 PARTIAL 场景。
