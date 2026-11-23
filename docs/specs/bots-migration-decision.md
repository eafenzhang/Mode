# Spec：`bots` 迁移路径裁决（LT-16.5 前置评估）

状态：**评估完成，P1 已采纳（2026-11-23，用户指令「按建议执行」）**。
本文记录证据与候选路径；P1 的两处口径改写已落地。

## 0. 结论

`bots` 与前面几个频道不同：它**不能**按「把频道搬进 Rust」处理，因为有三份现行裁决
互相冲突。必须先选路径，否则任何实现都会违反其中一份。

| 裁决 | 出处 | 说的是 |
| --- | --- | --- |
| A. bots 域继续由 Node Host 持有，不做频道迁移 | `docs/specs/im-sdk-runtime.md` §1（2026-11-20 采纳） | 飞书/企微用官方 npm SDK，钉钉/微信用手写 TS |
| B. bots 属于「live，需迁移」 | `docs/plans/2026-11-22-lt16-node-host-retirement.md` §16.5 盘点 | 删除 Node Host 前需迁 6 个 live 频道，bots 在列 |
| C. 终态唯一保留的 JS 执行面是插件负载的嵌入引擎 | `docs/plans/2026-10-29-full-rust-rewrite.md` §1 / DR-01 | 「核心发运集里不再有 Node 运行时」，JS 面只留第三方负载 |

A 与 B 直接冲突（留 vs 迁）；C 决定了「迁」的形态（重写成 Rust vs 变成嵌入 JS 引擎的负载）。

## 1. 证据

### 1.1 bots 依赖官方 npm SDK（第三方 JS，不可改写）

- `@larksuiteoapi/node-sdk@1.64.0`（`packages/services/src/bots/providers/feishuProvider.ts:1678`、
  `wecomChannelRuntime.ts`、`wecomSdk.ts` 等引用）
- `@wecom/aibot-node-sdk@1.0.7`
- 钉钉/微信无 npm SDK，是手写 TS（`dingtalkChannelRuntime.ts` 15k、`weixinChannelRuntime.ts` 12.6k）

按 C 的口径，这两个 SDK 属于「第三方 npm/TS 负载，版权与兼容性都不允许改写」——
与插件负载同类。**所以 bots 的技术归宿可能不是 Rust 重写，而是「宿主 Rust + 嵌入 JS 引擎」
承载这两个 SDK。**

### 1.2 体量与形状

| 项 | 规模 |
| --- | --- |
| `botsService.ts` | 393 KB / **9069 行** |
| `IBotsService` 方法数 | **47**（含 3 个事件） |
| UI 实际消费的方法（去重） | **33** |
| 同目录其它文件 | `messages.ts` 28.5k、`wecomChannelRuntime` 18k、`botRemoteWorkspaceBridge` 17k、`replyFormatter` 15.6k、三通道 runtime 各 12–15k |

### 1.3 已备的 Rust 底座只覆盖「入站解析」

- `crates/bots-core`（`adapter.rs` 6.4k + `types.rs` 8.3k）：provider 中立类型 +
  `ProviderAdapter` trait，是 B1 seam。
- `crates/bots-adapters`（钉钉 11k / 飞书 17k / 企微 11k / 微信 23k）：**入站解析**投影。
- `crates/{dingtalk-stream,weixin-ilink,feishu-ws,wecom-ws}`：协议参考。
- **这些 crate 没有任何产品依赖**：`grep` 全部 Cargo.toml，除自身外零引用；
  `host-channels` / `mode-server` 都不依赖它们。它们在 `[workspace] members` 里，
  但只参与编译，不进产品路径（`im-sdk-runtime.md` §1 的裁决结果）。
- **出站发送路径（B2c）从未做完**：`bots-domain-migration.md` §2 标注 B2c 待做，
  且 B3/B4/B5 已划掉。

### 1.4 配置/状态的事实源是三个 JSON + 凭据

`packages/services/src/bots/repo.ts` + `config.ts`：

```
{appConfigDir}/bot-config.v3.json     # 版本 3，bots[]
{appConfigDir}/bot-state.v3.json      # 版本 3，bots: Record<botId, channelState>
{appConfigDir}/bot-bindings.v3.json   # 版本 2，workspaceKey -> botId[]
凭据：bot:<id>:credential / bot:<id>:webhook-secret
```

`botConfigSchema` / `botsStateFileSchema` 都是 `.strict()`，且带 legacy 迁移
（`migrateBotsStateFileV3`、`importLegacyBotConfig`、`stripRetiredBotProviders`）。
Rust 侧目前**没有任何**这三个文件的读写实现。

### 1.5 「迁 bots」的实际工作量估

- 纯读面（getStatus/getConfig/listBots/getBotStates/listWorkspaceRefs/
  listBotWorkspaceBindings/listBotTaskBindings…）≈ 3 个 JSON schema 的 Rust 镜像
  + 迁移逻辑，中等；
- 写面（saveBot/deleteBot/saveConfig/各 block/approve/reject…）还要接管三个文件的
  **写所有权**与文件锁，中等偏大；
- 通道运行时（feishu-ws / wecom 长连 / weixin 轮询 / dingtalk stream + 心跳 + 重连 +
  ack 队列）**全部绑在官方 SDK 上**，Rust 重写等于放弃 A 裁决，回到「自实现四通道」——
  那正是 2026-11-20 明确停止的路线；
- 回复编排（`replyFormatter`、`outboundDelivery`、`groupHistory`、命令解析、权限）≈ 5k 行，
  强耦合会话运行时。

结论：**只迁配置面 = 半个频道**（不可行）；**全迁 = 放弃官方 SDK**（A 裁决的反面）；
**保持 = 与 B 冲突**。

## 2. 候选路径

| 路径 | 内容 | 代价 | 与裁决的关系 |
| --- | --- | --- | --- |
| **P1. 维持 A（推荐）** | bots 永久留在 Node sidecar；把 sidecar 变成「插件式 JS 宿主」的一部分（与插件负载同一种运行时） | 0（仅文档）；W6 删除窗口把 bots 排除在外 | 需要改写 B 与 C 的口径：C 的「唯一 JS 面是插件负载引擎」扩成「插件负载 + IM SDK 负载」 |
| P2. 全量 Rust 重写 | 放弃官方 SDK，自实现四通道 + 9k 行服务层 | 很高；且直接推翻 2026-11-20 的改向 | 满足 B 与 C，违反 A |
| P3. 配置面先迁、通道留 Node | Rust 接管三个 JSON 与凭据，通道仍 Node | 中；但会形成跨进程双写，违背单一所有者 | 不推荐（半个频道） |
| P4. 嵌入 JS 引擎承载 SDK | Rust 宿主 + 嵌入式 JS（如 QuickJS/Boa）跑 SDK，bots 服务层逐步 Rust 化 | 高，但符合 C 的方向 | 满足 B 与 C，需重定义 A |

## 3. 建议

**P1**，理由：

1. bots 是唯一一个「产品行为依赖第三方 JS SDK 且厂商还在演进」的域；重写会持续追
   wire 漂移（这正是 2026-11-20 的结论）；
2. W6 的目标是「核心发运集里没有 Node 运行时」，而 bots 的两个 SDK **本来就是 JS**，
   它们要活在某个 JS 运行时里；
3. P4 是中期正解，但需要先有嵌入引擎（`full-rust-rewrite.md` DR-01 已把它列为
   「承载从随包 Node 改为嵌入 JS 引擎」的独立工作），应当与插件宿主一起做，而不是
   在 bots 频道里单点起一个引擎。

### 3.1 P1 落地记录（2026-11-23，已采纳）

| 改动 | 文件 |
| --- | --- |
| `live，需迁移` 清单去掉 bots；`剩余 live 频道` 段落补 P1 裁决与依据 | `docs/plans/2026-11-22-lt16-node-host-retirement.md` |
| 结论行：删除前置从「5 个 live 频道」改为「3 个 live 面 + 1 个并入项」，bots 移出 | 同上 |
| §1 的 JS 面表述扩成「插件/MCP/Skills 负载 + IM SDK 负载，同一嵌入引擎」 | `docs/plans/2026-10-29-full-rust-rewrite.md` |
| W6 行：`bots` 移出删除前置并注明 P1 | 同上 |

**16.6 删除窗口的新口径**：删 Node Host 时 **不删 bots**——它的两个 SDK 与 bots 服务层
继续由 sidecar 提供，直到嵌入引擎就位后再整体换宿主。这意味着 16.6 的「Node 运行时
零残留」目标要分两步：先删 host-standalone 的 RPC/宿主编排，再等嵌入引擎替掉
sidecar 的 JS 宿主角色。

## 4. 未验证项（如实标注）

- 未实测飞书/企微 SDK 在嵌入 JS 引擎（QuickJS 等）下的可用性——它们依赖 `node:net` /
  `node:http` / `node:crypto`，很可能**不能**在纯嵌入引擎里跑，而需要一个完整的 Node
  兼容运行时。这是 P4 的最大未知项，也是把 P1 作为推荐路径的原因之一。
- 本文未评估「bots 只保留远程 workspace 桥接、砍掉 IM 通道」的产品取舍（那属于
  LT-0 决策轨道）。
