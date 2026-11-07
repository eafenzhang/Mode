# Spec：Coding Plan 可用性 HTTP 编排（Rust `coding-plan-http`）

状态：2026-11-05 第一版。
真源：`packages/services/src/model-provider/codingPlanProviderAvailability.ts` 与
`bigmodel/codingPlanEntitlement.ts`。

## 1. 范围

| Rust 入口 | TS 对应 |
| --- | --- |
| `CodingPlanAvailability::subscription_availability` | 个人订阅列表可用性 |
| `CodingPlanAvailability::start_plan_availability` | Start Plan balance 可用性 |
| `CodingPlanAvailability::team_entitlement` | 团队订阅详情 |
| `CodingPlanAvailability::team_availability` | customerInfo 团队项目校验 + 团队订阅详情 |
| `CodingPlanHttpPort` / `HttpRequest` / `HttpResponse` | 注入的 GET 传输 |
| `ReqwestCodingPlanHttp` | 生产 reqwest + rustls 传输（安装 ring provider） |

生产传输由 `ReqwestCodingPlanHttp::new(timeout)` 提供；代理/CA 由宿主按进程 env 配置，
本 crate 只负责请求构造、状态分类与解析接线。

## 2. 行为

- 个人订阅：
  - Authorization trim 后为空 → `Unknown`（不发请求）；
  - 请求头 `Authorization: <原值>`；
  - 401/403 → `Unavailable / coding_plan_auth_failed`；
  - 其它非 2xx / 传输错误 / JSON 解析失败 → `Unknown`；
  - 2xx → `coding_plan_entitlement::resolve_subscription_list_availability`。
- Start Plan：
  - Authorization 为空 → `Unavailable / coding_plan_not_authenticated`（不发请求）；
  - 401/403 → `coding_plan_auth_failed`；其它失败 → `Unknown`；
  - 2xx → `zai_start_plan_billing::resolve_start_plan_availability(payload, now_seconds)`。
- 团队可用性：
  - 空 token → `not_connected`；
  - customerInfo 必须存在匹配 org/project 且 `projectType === 2`，否则 `not_entitled`；
  - 订阅详情：Z.AI `Bearer <token>`，BigModel 原样 token；带 org/project 头；
    401/403 → `auth_failed`；不可用 → `not_entitled`。
- 团队订阅详情：
  - URL = `host.trim_end_matches('/') + /api/biz/team/subscribe/product/querySubscribeDetail`；
  - 请求头 `Authorization`、`bigmodel-organization`、`bigmodel-project`；
  - 非 2xx/传输/解析失败 → `Unknown`；2xx → 团队权益解析。

## 3. 差异

1. 401/403 之外的网络错误只归为 `Unknown`；宿主端口可提供更细的错误码。
2. 请求 `Authorization` 原样传递（调用方负责 `Bearer` 前缀与 key 归一化）。

## 4. 验收

- `cargo test -p coding-plan-http`：11 条（订阅成功/401/空 key、传输与非 2xx 未知、
  Start Plan 成功/缺凭据/403、团队 URL 与请求头、团队过期、团队可用性/项目校验/403、
  reqwest 本地回环）。
