# Spec：Provider 模型列表请求构造与响应解析（Rust `provider-model-listing`）

状态：2026-11-04 第一版（`packages/provider/src/model-listing.ts` 纯函数子集）。
真源：`packages/provider/src/model-listing.ts`。

## 1. 范围与所有者

| Rust 入口 | TS 对应 |
| --- | --- |
| `build_remote_model_list_request` | `buildRemoteModelListRequest` |
| `parse_remote_model_list_page` | `parseRemoteModelListPage` |
| `RemoteModelCatalogRequest` / `RemoteModelListRequest` / `RemoteModelListPage` | 同名类型 |

- 只迁移请求构造与响应解析；HTTP 执行、分页循环与 UI 编排不迁
  （services `remoteModelCatalog.ts` 的执行器仍归 TS）。
- 所有者是「模型检测」功能面：设置页用草稿连接信息（未保存的 baseUrl/apiKey/
  headers）构造一次可执行请求；响应按 `{ data: [...] }` 标准形状解析。
  URL/头语义必须与正式模型执行链一致，否则会出现「检测通过但正式执行失败」的
  语义分叉。

## 2. 行为

### 2.1 请求构造

- base URL trim 后为空 → `Base URL 不能为空`；不可解析 → `Invalid URL`；
  协议非 http/https → `Base URL 协议不受支持: <scheme>:`（TS `url.protocol` 带冒号）。
- openai 系（`openai-chat-completions` / `openai-responses`）：baseURL 原样使用，
  不插入 `/v1`；path 去尾部 `/` 后追加 `/models`；有 apiKey 时加
  `Authorization: Bearer <key>`。
- `anthropic-messages`：path 去尾部 `/`，小写判断不以 `/v1` 结尾则补 `/v1`
  （保留原大小写），再追加 `/models`；`searchParams.set("limit", "1000")` 语义 =
  保留其它既有参数、同名替换。Rust 侧先清空 query 再整体重写（`url` crate 的
  `query_pairs_mut` 是追加语义，直接用会重复既有参数）。
- 头：apiKey 存在时 anthropic 依次生成 `x-api-key`、`anthropic-version: 2023-06-01`
  与 `Authorization: Bearer`；显式 headers 拥有最高优先级，检测头不覆盖同名
  （大小写不敏感）显式头。
- apiType 非法值与 TS default 分支一致显式报错：`不支持的 Provider API 类型: <值>`。

### 2.2 响应解析

- 只接受对象且 `data` 为数组；其余形状分别报
  `Provider 模型列表响应格式无法识别` 与 `Provider 模型列表响应缺少 data 数组`。
- 条目 `id` 非字符串或 trim 后为空忽略；其余取 trim 后的字符串。
- `cursor` 仅在 `has_more === true` 且 `last_id` 为非空字符串时存在
  （Anthropic `after` 游标）；`has_more` 非 true 或 `last_id` 空串/缺失时不产出。

## 3. 已知差异

1. `apiType` 在 TS 是编译期联合类型，Rust 按运行时字符串处理（夹具覆盖非法值分支）。
2. URL 解析用 WHATWG 规范的 `url` crate，与 JS `new URL` 同规范；IDN/百分号编码的
   极端输入未纳入夹具。
3. headers 用 `BTreeMap` 承载（JSON 对拍按对象比较，与键序无关）。

## 4. 验收

- `pnpm provider-model-listing:fixtures` →
  `crates/provider-model-listing/fixtures/model-listing.json`
  - build 13 条：openai 尾斜杠/根路径/无 key、显式头优先、anthropic 补 `/v1` 与
    大小写保留、既有 query 保留 + limit 替换、显式头覆盖、空 baseURL/非法协议/
    非法 URL/非法 apiType 错误文案；
  - parse 10 条：id 过滤与 trim、cursor 出现/缺失语义（无 last_id、空串、
    `has_more:false`）、形状错误四类。
- `cargo test -p provider-model-listing`（TS 回放 2 + 单测 6）。
- `cargo clippy -p provider-model-listing --all-targets -- -D warnings`。
