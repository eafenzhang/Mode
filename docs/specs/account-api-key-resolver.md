# Spec：OAuth → Coding Plan API Key（Rust `account-api-key-resolver`）

状态：2026-11-05 第一版。
真源：`packages/services/src/model-provider/accountProviderApiKeyResolver.ts`。

## 1. 范围

| Rust 入口 | TS 对应 |
| --- | --- |
| `AccountApiKeyResolver::resolve` | `resolveProviderApiKey` |
| `AccountApiKeyResolver::resolve_team_key` | `resolveAccountTeamPlanRuntimeApiKey`（Key 部分） |
| `pick_org_and_project` | `pickOrgAndProject` |
| `successful_data` | `AccountProviderApiClient.fetchRemoteData` 的信封判定 |
| `AccountApiHttpPort` | 注入的 GET/POST 传输 |
| `ReqwestAccountApiHttp` | reqwest + rustls 生产传输 |

## 2. 行为

1. 空 token → `None`。
2. `getCustomerInfo` → 只接受成功业务码（缺省/`0`/`200`）并取 `data`。
3. 选机构/项目：排除 `projectType` 为 2 的 Team 项目；优先名称含 `默认机构` /
   `默认项目`，否则第一项；缺 `organizationId`/`projectId` → `None`。
4. `.../api_keys` 列表：找 `name === "mode-api-key"`；没有则 `POST {name}` 创建。
5. `.../api_keys/copy/<urlencoded apiKey>` 取 `secretKey`：
   - 有 secret → `"<apiKey>.<secretKey>"`；
   - 无 secret：BigModel 返回裸 `apiKey`；Z.AI 必须 secret → `None`。
6. 传输错误、非 2xx、JSON 非法、业务码失败 → `None`（与 TS catch 语义）。
7. Z.AI 请求头 `Authorization: Bearer <accessToken>`；BigModel 原样透传。

## 2b. 团队 Key

`resolve_team_key(provider, token, organizationId, projectId)`：先校验 customerInfo 中
org/project 存在，再 list `name=mode-team-api-key && keyType=2` 的 Key，缺失则 POST
`{name,keyType:2}`，最后 copy secret；返回 `"<key>.<secret>"` 或裸 key。请求头为
`Authorization: <token>` + `bigmodel-organization/project`。

## 3. 差异

1. 未实现 token 刷新；调用方传入的 access token 直接使用。
2. 未做响应 schema 的严格逐字段校验；缺字段按 `None`，多余字段忽略。

## 4. 验收

- `cargo test -p account-api-key-resolver`：8 条（选机构/项目、BigModel 裸 Key、
  Z.AI secret 组合、Z.AI 缺 secret、空列表创建、团队 Key 创建/缺项目、业务失败）。
