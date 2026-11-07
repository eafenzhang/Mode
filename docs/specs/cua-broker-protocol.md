# Spec：CUA broker 行协议装帧与 socket 路径铸造（Rust `cua-raw-input::broker_protocol`）

状态：2026-11-04 第一版（`packages/mode-cua/broker.js` 纯逻辑子集）。
真源：`packages/mode-cua/broker.js`；TS 侧行为测试见 `packages/mode-cua/test/broker.test.mjs`。

## 1. 范围

| Rust 入口 | TS 对应 |
| --- | --- |
| `parse_request_line` | `parseRequestLine` |
| `ok_response` / `error_response` / `error_response_from_failure` | `okResponse` / `errorResponse` / `errorResponseFromException` |
| `serialize_response` | `serializeResponse` |
| `dispatch_request` / `handle_request_line` | `dispatchRequest` / `handleRequestLine` |
| `is_broker_method` / `is_read_only_broker_method` | `isBrokerMethod` / `isReadOnlyBrokerMethod` |
| `mint_broker_socket_path` / `resolve_broker_socket_path` | `mintBrokerSocketPath` / `resolveBrokerSocketPath` |
| `BROKER_METHODS` / `READONLY_METHODS` / 两个 env 常量 | 同名常量 |

- **唯一所有者**：协议装帧与路径铸造归本模块；socket 传输（`net.connect`、超时、
  连接生命周期）与 helper host 进程归宿主装配层，本模块不做 IO。
- 随机源与平台注入：`mint_broker_socket_path(random_hex, dir, tmp_dir, is_windows)`
  的 `random_hex` 对应 TS `randomBytes(8).toString("hex")`（16 hex，路径即凭据）。

## 2. 行为

### 2.1 行协议

- `parse_request_line`：JSON 非法 / 非对象 / `method` 非字符串 → None；
  `params` 缺失或 null 按 `{}`；`id` 非字符串按 null。
- `dispatch_request`：方法表外先拒绝（`method_not_found`，文案
  `unknown method: <method>`）；backend 抛错收敛为
  `errorResponseFromException` 语义（字符串 `code` 保留，空/缺失 → `internal`）。
- `handle_request_line`：装帧非法按 `invalid_request`（`malformed request`）。
- 方法表 12 项；只读表 6 项——`raw_input`、`perform`、`rd_media_*`、`launch_app`
  是写路径，故意不在只读表内（只读面不得触达注入）。
- 响应序列化是紧凑 JSON，不含行尾换行（写入方补 `\n`）。

### 2.2 socket 路径

- 铸造：win32 无 `dir` → `\\.\pipe\mode-cua-<hex>`；其余落 `<dir ?? tmpdir>`，
  分隔符随平台；尾部分隔符不重复。
- env 发现（`MODE_CUA_PERMISSION_BROKER_SOCKET`）：注入值**原样返回不做 trim**
  （TS 测试锁定 `" X "` 原样返回）；空白值不算注入，回落铸造——services 的
  `probeStableCuaHelperSocket` 与 macOS 懒启动注入都依赖这条语义。

### 2.3 socket 传输与健康探测（2026-11-05）

`BrokerClient::call(method, params, timeout_ms)`：单请求单连接，写入
`{"id":"1","method":…,"params":…}\n`，读到首个换行后解析：

- `{ok:true,result}` → `result`；
- `{ok:false,error:{message,code}}` → `BrokerError{code}`；
- 响应 JSON 非法 → `internal`；连接失败/对端关闭 → `stale_socket`；
- 超时 → `timeout`；单个响应行上限 **4 MiB**（超出 → `internal`，防止无界缓冲）。

平台连接：POSIX 用 Unix domain socket，Windows 用命名管道（`tokio::net`）。

`transport::broker_call(runtime)` 把异步客户端包成注入端口的同步 `BrokerCall`：
构造 `params = {"events": events}` 调 `raw_input`，把 `dispatched` 转成
`RawInputBrokerResult`；`BrokerError` 原样映射为 `BrokerCallError::Broker`。
调用线程会 `block_on(runtime)`，**不得**从该运行时的 worker 线程调用；宿主应把它
用于同步桥或专用阻塞线程。
`BrokerClient::probe_health(options)` 在 deadline 内按 100ms 间隔、每次 500ms 超时
轮询 `health`；只接受 `pid === null` 或数字的结果，返回
`{bundleId: string|null, pid: number|null}`；耗尽 deadline → `broker_unavailable`。
默认总超时 10s，与 TS 一致。

## 3. 已知差异

1. 响应序列化：TS `JSON.stringify` 按插入序，Rust 对拍按解析后的 JSON 值比较
   （键序无关）。
2. `dispatch_request` 的 backend 形态是 `BackendOutcome`（Ok/Err{message, code}），
   不承载 TS 异常对象；`code` 非字符串或空串一律 `internal`。
3. 响应行有 4 MiB 上限（TS 无上限）；正常 broker 响应远小于该值。

## 4. 验收

- `cargo test -p cua-raw-input`：broker_protocol 7 条（行编解码与垃圾拒绝、
  方法表白名单与只读子集、未知方法/包装 backend 失败、无 code → internal、
  malformed → invalid_request、平台铸造与 dir 覆盖、env 原样返回与回落铸造）；
  连同既有 injector 8 条与 resolver 7 条全绿。
- transport 用例：成功结果、远端错误、非法 JSON、超时、健康探测成功与超时、
  `broker_call` 的 `dispatched` 解析与错误映射；Windows 用命名管道 server、
  Unix 用 UnixListener 作为对端。
- TS oracle：`packages/mode-cua/test/broker.test.mjs` 保留为行为真源。
- `cargo clippy -p cua-raw-input --all-targets -- -D warnings`。
