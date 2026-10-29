# Spec：system 频道（`crates/system-service`）

状态：2026-10-29 实现（W3-2 的一部分）。行为真源：

- `packages/services/src/system/system.ts`（三个方法）
- `packages/services/src/system/systemService.ts`（内网探测）
- `packages/services/src/system/integratedTerminalShells.ts`（Windows shell 列表）
- `packages/shared/src/protocol.ts` 的 `SystemInfo` / `IntegratedTerminalShellOption`
- `packages/shared/src/intranetProbe.ts` 的全部探测类型

## info

```json
{ "homedir": "<os.homedir()>", "platform": "<process.platform>" }
```

`homedir` 与 Node 同口径：Windows `USERPROFILE` → `HOMEDRIVE`+`HOMEPATH`；Unix `HOME`；
都取不到时为空串。`platform` 做 `windows→win32` / `macos→darwin` 映射。

## listIntegratedTerminalShells

非 Windows 平台**恒返回空数组**（TS 同样短路）。Windows 上：

1. 第一个选项恒为 CMD：`ComSpec`（大小写不敏感，trim 后非空）否则 `cmd.exe`；
   `id = "cmd:<path>"`，`source = "system"`。
2. Git Bash：
   - 先查固定路径 `C:\Program Files\Git\bin\bash.exe`、`C:\Program Files (x86)\Git\bin\bash.exe`；
   - 再在 `PATH`（按 `;` 切分，跳过空白项）里按 `PATHEXT` 找 `git<ext>`；
   - 对找到的 `git.exe` 推断 `../bin/bash.exe` 与 `../../bin/bash.exe`（win32 语义的
     join/normalize）；
   - 命中即返回 `{dialect:"git-bash", id:"git-bash:<path>", label:"Git Bash", source}`，
     固定路径命中是 `system`，PATH 推断命中是 `path`。
3. 可执行判定：注入的 `isExecutable` 优先；否则看目标是否是普通文件
   （对应 Node `accessSync(X_OK)` 在 Windows 上的实际效果）。

`PATHEXT` 缺失时默认 `[".COM",".EXE",".BAT",".CMD"]`，全部小写；若列表里没有 `.exe`，
把 `.exe` 插到最前。

## probeIntranet

### 归一化

| 字段 | 规则 |
| --- | --- |
| `targets` | 逐个归一化；归一化失败的目标被**丢弃**（不报错） |
| `host` | trim 后为空 → 丢弃 |
| `port` | 整数且 `1..=65535`；否则 `22` |
| `url` | trim 后为空或不是 `http`/`https` → 丢弃；用 WHATWG 序列化后的 URL |
| `timeoutMs` | 有限数 → `floor` 后夹到 `[100, 10000]`；否则 `800` |
| `attempts` | 有限数 → `floor` 后夹到 `[1, 3]`；否则 `2` |
| `requiredSuccessCount` | `totalTargets == 0` → `1`；否则有限数 → `floor` 后夹到 `[1, total]`；否则 `1` |
| `id` | trim 后非空；否则 tcp 用 `host:port`，service 用序列化后的 URL |
| `expectedMarker` / `token` | trim 后非空才生效 |

### 探测

- 每个目标独立重试，最多 `attempts` 次；不同目标之间并发（`Promise.all` 语义）。
- TCP：连接成功返回耗时毫秒；超时报 `timeout(<ms>ms)`；其它错误取最后一次的错误文本。
- service：`GET url`，`token` 非空时带 `x-mode-intranet-token` 请求头；
  - 非 2xx → `HTTP <status>`；
  - 响应体必须是 `{ok: boolean, marker?: string}`；结构不合法 → `invalid service response[...]`；
  - `ok !== true` → `service returned ok=false`；
  - `expectedMarker` 存在且与响应的 `marker` 不等 →
    `marker mismatch(expected=<期望>, actual=<实际或 <empty>>)`；
  - 超时 → `timeout(<ms>ms)`。
- 结果里的 `latencyMs`：可达时是本次成功尝试的耗时；不可达时 `null`。
- 聚合：`isIntranet = total > 0 && reached >= requiredSuccessCount`；
  `strategy`：全 tcp → `tcp-connect`，全 service → `service-http`，混合 → `mixed`；
  空目标按 `every` 语义落到 `tcp-connect`。`checkedAt` 是毫秒时间戳。

## 已知差异

| 差异 | 说明 |
| --- | --- |
| 连接失败的错误文本 | Node 是 `connect ECONNREFUSED 127.0.0.1:1`，Rust 是 `std::io::Error` 文本；只进 `error` 诊断字段，不是契约 |
| HTTP 客户端 | `reqwest` + `rustls`（平台证书校验）；Node `fetch` 用内置根证书，信任面可能略有差异 |
| 重定向上限 | `reqwest` 默认 10 次，Node `fetch` 20 次 |
| homedir 兜底 | 两个环境变量都缺失时 Rust 返回空串，Node 会再查系统用户目录 |
| URL 序列化 | 两边都是 WHATWG 兼容实现；极端输入的空格/默认端口规范化可能有细微差异 |

## 验收

- `crates/system-service/src/shells.rs` 单元测试（7 条）：非 Windows 空数组、ComSpec
  大小写与 trim、固定 Git Bash 路径、PATH 搜索 + PATHEXT、可执行判定、win32 join。
- `crates/system-service/src/service.rs` 单元测试（3 条）：homedir 优先级与空值回落、
  注入环境/平台的 info。
- `crates/system-service/tests/intranet_probe.rs`（7 条）：默认值、非法目标丢弃、
  attempts/timeout/port/id 夹取、重试次数、marker 不匹配、service 成功、阈值与空目标。
- `crates/host-channels/tests/system_channel.rs`（6 条）：三个方法的参数/返回形状与
  camelCase 字段名。
- 频道名漂移守卫覆盖 `system`。
