# Spec：RPC 线格式（`@mode/rpc` 二进制序列化）

状态：2026-10-29 建立。对应实现真源：`packages/rpc/src/serialization.ts`（TS，现役）。
Rust 侧对拍实现：`crates/rpc-wire`（只做编解码，不含传输与频道层）。

## 为什么需要这份 spec

M4b（Host 并入壳）要求把 Host 的服务面搬到壳的 tokio 运行时里，renderer ↔ Host 的
RPC 帧就不再经过「Node 子进程 + stdio」，而是由壳直接编解码。壳是 Rust，所以必须有一份
**与 TS 实现逐字节一致**的线格式契约；否则同一条消息在两条路径上的字节不同，scoped
attachment、帧解码与现有对拍都会失效。

这份 spec 只描述**字节形态**，不描述频道语义（频道在 `packages/shared/src/channels.ts`，
命令面在 `docs/specs/tauri-platform-commands.md`）。

## 进程分帧（stdio）

Host sidecar ↔ 壳的帧 = `[4 字节大端无符号长度][payload]`，payload 就是上面的
`serialize` 结果，两侧都不解释它。

- 单帧上限 **64 MiB**（`MAX_FRAME_BYTES`）：长度前缀是攻击面，超限即判定该流不可恢复——
  后续字节全部丢弃、只报一次，不再尝试重新对齐。
- 收方向必须自己处理粘包/分片：够 4 字节才读长度，够 `4 + length` 才消费，不能在 body
  未到齐时提前消费首部。

## 协议消息头（`SocketProtocol`，13 字节）

用于 socket/MessagePort 一类流式传输：`type(1) + id(4 BE) + ack(4 BE) + length(4 BE)`，
随后是 `length` 字节的 payload。

| type | 名称 |
| --- | --- |
| 0 | None |
| 1 | Regular |
| 2 | Control |
| 3 | Ack |
| 5 | Disconnect |
| 6 | ReplayRequest |
| 7 | Pause |
| 8 | Resume |
| 9 | KeepAlive |

只有 `Regular` 会把 payload 向上抛；`length = 0` 的 `Regular` 抛一个空帧。未知 type
**不报错**（TS 侧是直接读出来再忽略），Rust 侧按原始 `u8` 保存并原样回写。

## RPC 信封

一条 RPC 消息的 payload = `serialize(header)` + `serialize(body)`，header 是数组：

| 请求类型 | 取值 | header | body |
| --- | --- | --- | --- |
| Promise | 100 | `[100, id, channel, method]` | 调用参数 |
| PromiseCancel | 101 | `[101, id]` | `undefined` |
| EventListen | 102 | `[102, id, channel, event]` | 订阅参数 |
| EventDispose | 103 | `[103, id]` | `undefined` |

`id` 按 int32 编码（JS number 走 Int 分支）。超出 int32 的 id 在 TS 侧会落到 JSON 回退，
属未定义行为，Rust 侧不复制。首元素不是这四个取值的帧不是合法 RPC 请求。

## scoped 帧与附件控制帧

只出现在**页面桥 ↔ Host** 这一段（单端口多路复用，`packages/rpc/src/frameMultiplexer.ts`）：

- **base 帧**：不带前缀，逐字节不变；
- **scoped 帧**：`[0xFF][scopeId 4 字节大端][payload]`。scope id 取值 1..=2³²−1，
  0 保留给 base，编码时非法值直接报错；marker 在但整帧不足 5 字节判为**畸形并被丢弃**；
- **控制帧**：`[0xFE][JSON]`，body 固定为
  `{"type":"scope-ready"|"scope-detach","scopeId":N}`。编码时键序固定为
  `type` → `scopeId`（与 TS 对象字面量序一致）；解析侧接受任意键序，形状不合法返回
  `null`（调用方丢弃）。控制帧先于 RPC 校验处理，不属于任何 scope。

## 序列化结构（payload 内部）

```
value := tag [length] payload?
tag   := 1 字节，取值见下表
length:= VQL 编码的非负整数（仅 String/Buffer/VSBuffer/Object/Array 使用）
```

| tag | 名称 | 语义 | 载荷 |
| --- | --- | --- | --- |
| 0 | Undefined | JS `undefined` | 无 |
| 1 | String | UTF-8 文本 | VQL 字节长度 + UTF-8 字节 |
| 2 | Buffer | JS `Uint8Array` | VQL 字节长度 + 原始字节 |
| 3 | VSBuffer | `@mode/rpc` 的 `VSBuffer` 包装 | VQL 字节长度 + 原始字节 |
| 4 | Array | JS 数组 | VQL 元素个数 + 逐个 `value` |
| 5 | Object | 其余值（JSON 回退） | VQL 字节长度 + UTF-8 的 `JSON.stringify` 结果 |
| 6 | Int | JS 32 位整数 | VQL 编码的整数 |

## 分派顺序（必须逐条保持）

`serialize` 按以下顺序判定，先命中先返回：

1. `typeof data === "undefined"` → Undefined；
2. `typeof data === "string"` → String；
3. `data instanceof VSBuffer` → VSBuffer；
4. `data instanceof Uint8Array` → Buffer；
5. `Array.isArray(data)` → Array；
6. `typeof data === "number" && (data | 0) === data` → Int；
7. 其余 → Object（JSON 回退）。

由此产生两条容易漏掉的行为，Rust 侧必须一致：

- **超出 int32 的数字不是 Int**：`2 ** 31`、`1.5`、`NaN` 都落到 Object 分支，分别编码为
  JSON 文本 `2147483648`、`1.5`、`null`。
- **负数 Int 按 32 位无符号量编码**：`-1` 是 `FF FF FF FF 0F`（5 字节），不是符号扩展的
  变长负数。

## VQL 编码

每个字节低 7 位是数据，最高位表示「后面还有字节」；按 7 位一组、小端顺序输出。

- 数值按 JS 的 32 位语义处理：编码前先转成 `u32`（`value as u32`），所以负数用满 32 位。
- 解码累计到 `u32` 后按 `i32` 解释，与 JS 的位运算结果一致。
- 合法输入最多 5 字节（35 位）；超过 5 字节按**解码错误**处理，不做截断——JS 实现在超过
  32 位后会发生移位回绕，那是未定义行为，不复制。

## Object 分支的 JSON 细节

- 使用 `JSON.stringify` 的默认形态（无空格、保留插入顺序、`undefined` 对象属性被丢弃、
  数组中的 `undefined` 变成 `null`）。
- 嵌套 `Uint8Array` 不展开成 `{"0":…}`：被替换为
  `{"__mode_rpc_nested_uint8array_v1":true,"base64":"<标准 base64>"}`，解码时再还原。
  该标记只在**恰好**两个键且标记为 `true` 时生效。
- Rust 侧把 Object 当作**不透明 JSON 文本**承载（`Value::Json`），`Value` 无法表达
  「对象里嵌套字节」，因此 rpc-wire **不做透明还原**。共享辅助
  `rpc_wire::nested_bytes` 提供与 TS 同口径的判定与编解码：
  `is_marker` / `from_marker` / `to_marker` / `base64_encode` / `base64_decode`。
  频道适配器对**已知二进制字段**显式使用该辅助（编码方向 `Value::to_json` 已统一走
  `to_marker`；解码方向由频道在解析参数时调用 `from_marker`）。
  未使用该辅助的频道不得声称已覆盖含二进制嵌套的消息。

## 对拍口径

`crates/rpc-wire` 的用例以 TS 实现产出的**黄金字节**为准：

```bash
pnpm rpc-wire:fixtures
cargo test -p rpc-wire
```

- 黄金文件：`crates/rpc-wire/tests/fixtures/values.json`（序列化样例）与
  `frames.json`（分帧、协议消息头、RPC 信封、scoped/控制帧），每条含名称与十六进制。
- 判定：Rust 编码对每个样例产出的字节必须与黄金字节**完全相等**；Rust 解码必须能把
  黄金字节解出等价结构并原样再编码回去。
- 新增 tag、改变分派顺序或改信封形状时，先改 TS 实现与本 spec，再重新生成黄金文件；
  不允许只改 Rust 让用例变绿。

## 非目标

- 不定义流控帧（`saturated`/`drained`）与附件生命周期语义（scope 何时 attach/detach 由
  `2026-10-19-lr4-attachment-spike.md` 与 `rpc-host-dispatch.md` 决定）。
- 不定义频道名、方法名与事件语义。
- 不承诺与上游 `zai-org/ZCode` 的线格式一致；这里的真源是本仓库的 TS 实现。
