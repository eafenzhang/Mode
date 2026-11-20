# IM SDK 集成与运行时在线升级

状态：v1（2026-11-20）。上位：`docs/plans/2026-10-29-full-rust-rewrite.md` §W5；
本文对该计划的「IM 四通道 Rust 自实现」作出方向修正。
关联：`docs/specs/bots-im-transport.md`（wire 事实，转参考资料）、
`docs/specs/bots-domain-migration.md`（B 方案，已作废）、
`docs/specs/tauri-updates.md`（应用整包更新）、
`docs/specs/clean-room-policy.md`。

## 1. 裁决：官方 SDK + 运行时在线升级

| 方案 | 结论 | 理由 |
| --- | --- | --- |
| A. Rust 自实现四通道传输层（W5 原路线） | **停止接入** | 厂商 wire 漂移的维护成本高于收益；飞书/企微连接生命周期、ACK 与重连语义由官方 SDK 承担更稳。已交付的 `crates/{dingtalk-stream,weixin-ilink,feishu-ws,wecom-ws,bots-core,bots-adapters}` 保留为协议参考资料，**不进入任何产品依赖路径**。 |
| B. Node Host 集成官方 npm SDK | **采纳** | 飞书 `@larksuiteoapi/node-sdk@1.64.0`、企微 `@wecom/aibot-node-sdk@1.0.7` 已在 `RUNTIME_EXTERNALS` 随包发运；钉钉/微信继续用现有手写 TS。bots 域继续由 Node Host 持有，不做 B4 频道迁移。 |
| C. SDK 运行时在线升级（本文新增） | **采纳** | 厂商 SDK 修复不必等整包发版：随包 SDK 是基线，在线安装的**版本化运行时**优先加载，校验失败或未配置时回退随包版本。 |

**非目标**：

- 不改写、不重打包 SDK 源码：升级包是官方 npm 安装产物的原样拷贝。
- 不做 SDK 私有 API 适配层之外的协议实现；钉钉/微信没有 npm SDK 依赖，本波不纳入运行时。
- 本波不做设置页 UI 与发布渠道配置：URL/公钥经环境变量或壳资源注入，UI 与 CI 发布后续接。

## 2. 所有权

| 事实 | 所有者 |
| --- | --- |
| `active.json` 活跃版本指针 | 升级器（`imSdkRuntime.ts`）唯一写者，临时文件 + rename 原子替换 |
| `versions/<version>/` 版本目录 | 安装完成后只读；运行期不得改写 |
| SDK 模块解析 | `packages/services/src/bots/imSdkRuntime.ts`，全仓库唯一入口 |
| 升级包清单与哈希 | 发布流水线；Node 侧只校验不生成 |
| 信任根（Ed25519 公钥） | 壳资源文件或 `MODE_IM_SDK_RUNTIME_PUBKEY`；Node 进程不持有私钥 |
| 应用配置根 | `getAppConfigDir()`（`{dataRoot}/v2`），与凭据/设置为同一数据根 |

升级运行时只属于本机：不写 workspace、不同步到远端、不随对话导出。

## 3. 磁盘布局与清单

```text
<appConfigDir>/im-sdk-runtime/
  active.json                       # { schemaVersion: 1, version, activatedAt, bundleSha256 }
  versions/<version>/
    manifest.json                   # 发布清单（验签对象）
    node_modules/@larksuiteoapi/node-sdk/...
    node_modules/@wecom/aibot-node-sdk/...
    node_modules/<transitive deps>/...
```

`manifest.json`：

```json
{
  "schemaVersion": 1,
  "version": "2026.11.20.1",
  "createdAt": "2026-11-20T00:00:00.000Z",
  "hostApiVersion": 1,
  "sdkVersions": {
    "@larksuiteoapi/node-sdk": "1.64.0",
    "@wecom/aibot-node-sdk": "1.0.7"
  },
  "entrypoints": {
    "@larksuiteoapi/node-sdk": "node_modules/@larksuiteoapi/node-sdk/index.js",
    "@wecom/aibot-node-sdk": "node_modules/@wecom/aibot-node-sdk/index.js"
  },
  "bundle": {
    "file": "im-sdk-runtime-2026.11.20.1.tar.gz",
    "size": 1234567,
    "sha256": "<hex>"
  },
  "files": [{ "path": "node_modules/...", "size": 123, "sha256": "<hex>" }]
}
```

约束：

- `version` 必须匹配 `^[0-9]+(\.[0-9]+)*$`，按点分段数值比较；`active` 存在时只接受**更大**版本（拒绝降级）。
- `hostApiVersion` 必须等于 `1`；不兼容的包不安装。
- `files` 是完整文件集：解包后多一个、少一个、大小或哈希不符都失败。
- `entrypoints` 的值必须出现在 `files` 中，且指向真实文件。它是**模块入口文件**
  （发布侧解析 `main`/`exports` 后固化），不是 `package.json`——避免 `require` 与
  `import` 条件在客户端解析出不同入口。加载器直接 `import(fileUrl)` 该文件。
- 清单签名是 **Ed25519 detached 签名**，覆盖 `manifest.json` 的原始字节；签名文件为
  `<manifest-url>.sig`（base64 原文，可带换行）。公钥为 base64 编码的 32 字节 raw key。

## 4. 加载顺序

`resolveImSdkEntry(packageName)`（同步、每次通道启动调用）：

```text
1. active.json 不存在 / 不可解析 / 版本目录缺失 / manifest 不合法 / entrypoints 缺失
     → 返回 undefined（回退随包 node_modules）
2. active 合法
     → 返回 <versions>/<version>/<entrypoint 的绝对路径>
```

调用方把该路径转成 file URL 后 `import()`；传递依赖从入口所在的 `node_modules` 树解析。
失败一律回退随包版本并 `warn` 一次：**可用的旧版本优于不可用**。
未配置升级（无 `active.json`）时行为与今天完全一致。

**生效边界**：

- 已建立的飞书/企微长连接继续持有旧模块（Node 模块缓存不卸载）；
- 新连接、通道停用后重启、Host 重启使用新版本；
- 激活后清空懒加载缓存（`loadWeComSdk` 的 promise 按解析结果缓存），不需要重启 Host。

## 5. 升级流程

```text
refresh（启动时可选 + 后续手动入口）
  └─ GET <manifestUrl>
       └─ GET <manifestUrl>.sig（缺省启用的前提：公钥已配置）
            └─ 验签 → 解析清单 → 版本/兼容性检查
                 ├─ 已安装且文件校验通过 → 确保 active 指针 → up-to-date
                 └─ 需要安装
                      ├─ GET bundle（限速/超时；先核对 size + sha256）
                      ├─ 解包到 versions/.staging-<随机>
                      ├─ 逐文件 sha256 + 清单完备性 + entrypoints 存在性
                      ├─ rename → versions/<version>（原子）
                      ├─ 写 active.json（tmp + rename）
                      ├─ GC：保留 active + 上一个 active（回滚目标）
                      └─ 清懒加载缓存 → activated
```

失败语义：任一步失败删除 staging，`active.json` 保持不变（回滚 = 指针不动或指回旧版本）；
下载中断不留半成品目录。网络失败不清除已安装版本，也不阻断 Host 启动。

## 6. 安全边界

- **无公钥 = 功能关闭**：缺 `MODE_IM_SDK_RUNTIME_PUBKEY`（或壳资源公钥）时不下载、不安装，
  不允许退化为「不验签」。
- 只接受 `https` 清单与包 URL（`MODE_IM_SDK_RUNTIME_*` 指向 `http://127.0.0.1`/`localhost`
  时仅供开发/测试）。
- 解包拒绝：绝对路径、`..` 段、反斜杠、盘符、符号链接、硬链接、设备/管道文件、
  超长路径；文件数上限 20,000，解压总大小上限 512 MiB（gzip 输出同样设上限）。
- 所有文件按清单校验；清单外文件视为失败。不执行包内脚本（无 npm install / postinstall）。
- 下载与解包全程不使用 `shell`；不信任 `Content-Length`，以实际字节与清单为准。

## 7. 配置项

| 变量 | 作用 |
| --- | --- |
| `MODE_IM_SDK_RUNTIME_MANIFEST_URL` | 清单地址；与公钥同时存在才启用在线升级 |
| `MODE_IM_SDK_RUNTIME_PUBKEY` | base64 raw Ed25519 公钥（32 字节） |
| `MODE_IM_SDK_RUNTIME_DISABLE` | `1`/`true`：kill switch，忽略 active 指针并回退随包 SDK |

壳资源形态（后续）：`<MODE_HOST_RESOURCES_DIR>/im-sdk-runtime/pubkey.txt` 与 `manifest-url.txt`。

## 8. 验收

- 无 `active.json`：两个通道照旧从随包 `node_modules` 加载，启动路径不加载 SDK（懒加载守卫不变）。
- 安装 v2 后新开通道使用 v2；旧连接不变；Host 重启后仍为 v2。
- 清单签名错误、签名缺失、公钥缺失 → 不安装，`active.json` 不变。
- 包内文件被篡改 / 清单外多文件 / 缺文件 / 大小不符 → 安装失败，staging 清理。
- 路径穿越（`../x`）、绝对路径、符号链接、超限文件数与体积 → 解包失败。
- 版本不递增或 `hostApiVersion` 不匹配 → 拒绝安装。
- 下载中断 / 断网 → 无 staging 残留，旧版本可继续加载。
- 删除 `active.json` → 回退随包 SDK；`MODE_IM_SDK_RUNTIME_DISABLE=1` 同样回退。
- GC 保留 active 与上一个 active（回滚目标）；上一个 active 不可用时保留最高旧版本。
- `pnpm typecheck`、`pnpm lint`、`pnpm --filter @mode/services test` 全绿。

## 9. 后续

1. 发布流水线：生产安装两个 SDK（含传递依赖）→ tar.gz → 生成清单 → Ed25519 签名 → 上传。
2. 设置页展示当前 SDK 版本/来源（随包/在线）与「检查更新」入口。
3. 壳资源注入 `manifest-url` / `pubkey`，替代环境变量。
4. 视需要把钉钉（如引入官方 SDK）与微信纳入同一运行时清单。
