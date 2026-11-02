# Spec：官方 MCP 鉴权常量与 origin 信任（Rust `official-mcp-auth`）

状态：2026-11-01 首版。
真源：`packages/shared/src/official-mcp-auth.ts`。

## 1. 范围

| Rust 入口                                      | TS 对应                                    |
| ---------------------------------------------- | ------------------------------------------ |
| 常量（auth type/provider/header/meta/env）     | 同名常量                                    |
| `read_mode_mcp_request_context_meta`           | `readModeMcpRequestContextMeta`            |
| `is_official_mcp_reserved_header_name`         | `isOfficialMcpReservedHeaderName`          |
| `find_official_mcp_reserved_headers`           | `findOfficialMcpReservedHeaders`           |
| `summarize_official_mcp_identity_headers`      | `summarizeOfficialMcpIdentityHeaders`      |
| `is_official_mcp_origin_trusted`               | `isOfficialMcpOriginTrusted`               |
| `OfficialMcpTrustedOriginRegistry::is_trusted` | `createOfficialMcpTrustedOriginRegistry`   |

纯函数；registry 的 Mode API origin 解析器通过 `ModeApiOriginResolver` 注入（async trait），
解析失败按不可信处理。

## 2. 保留头与摘要

- 保留头（小写比较、trim）：五个身份头 `authorization` / `x-bigmodel-authorization` /
  `bigmodel-target-type` / `bigmodel-organization` / `bigmodel-project`，加上
  `x-coding-plan-api-key`、`mcp-session-id`、`mcp-protocol-version`。
- `find` 去重后按字节序排序。
- 身份摘要只输出 header 名（排序）、Organization/Project 是否存在、两者是否成对、
  以及非空 `bigmodel-target-type`；不输出任何值。
- 请求上下文 `_meta` 读取新键 `com.mode/request-context`，`null`/缺失时回退旧键
  `com.zcode/request-context`。

## 3. Origin 信任

1. `origin` trim 后为空 → `invalid_input`。
2. 若 origin 归一化为 http loopback（`127.0.0.1`/`localhost`/`[::1]`，无凭证），
   与逗号分隔的 dev 白名单逐项归一化比较，命中 → `ok`。
   **必须先确认目标是 loopback**，否则两侧都是 `None` 会放行任意 origin。
3. 期望 origin = `modeApiOrigin` 归一化为 https origin（无 username/password）；
   缺失/非法/http → `mode_origin_unresolved`。
4. 目标 origin 归一化为 https origin 后与期望不等 → `origin_mismatch`；相等 → `ok`。
5. 归一化只取 URL origin（路径/查询/片段被丢弃）；带凭证的 URL 一律拒绝。

`plugin_id` 只用于日志/凭证归属，不参与判定。

## 4. 已知差异

1. `url` crate 与 WHATWG `URL` 的极端输入（非常规 scheme、非 ASCII 主机）解析细节可能
   不同；https/loopback 与凭证拒绝行为一致。
2. registry 用原生 async trait，不使用 TS 的 Promise；解析失败语义相同（不可信）。
3. 排序使用字节序（TS 对 ASCII header 名等价）。

## 5. 验收

- `pnpm official-mcp-auth:fixtures` → `crates/official-mcp-auth/fixtures/official-mcp-auth.json`
  （9 个 header 判定、3 组 find、4 组摘要、5 组 meta、17 组 origin 信任、5 组 registry）。
- `cargo test -p official-mcp-auth`（同步 + tokio registry 用例）。
- `cargo clippy -p official-mcp-auth --all-targets -- -D warnings`。
