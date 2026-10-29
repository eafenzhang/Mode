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

## 帧骨架

一条 RPC 消息 = 一次 `serialize()` 调用写出的字节串；多帧在 stdio 上以 **4 字节大端长度
前缀**分隔（`packages/desktop/src/host-standalone/stdioFrames.ts`）。长度前缀与 scoped
attachment 头不属于本 spec 的 `serialize`/`deserialize`。

`serialize` 的递归结构：

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
- Rust 侧本轮把 Object 当作**不透明 JSON 文本**承载（`Value::Json`），不做嵌套字节还原；
  还原规则登记为实现差距，见 `docs/plans/2026-10-29-m4b-host-into-shell.md`。在还原实现前，
  Rust 侧不得声称已覆盖含二进制附件的消息（skill sync / 附件传输）。

## 对拍口径

`crates/rpc-wire` 的用例以 TS 实现产出的**黄金字节**为准：

```bash
pnpm rpc-wire:fixtures
cargo test -p rpc-wire
```

- 黄金文件：`crates/rpc-wire/tests/fixtures/values.json`（每条含名称、值描述、十六进制）。
- 判定：Rust `serialize` 对每个样例产出的字节必须与黄金字节**完全相等**；Rust `deserialize`
  必须能把黄金字节解出等价结构并原样再编码回去。
- 新增 tag 或改变分派顺序时，先改 TS 实现与本 spec，再重新生成黄金文件；不允许只改 Rust
  让用例变绿。

## 非目标

- 不定义帧外的长度前缀、scoped attachment 头与流控帧（`saturated`/`drained`）。
- 不定义频道名、方法名与事件语义。
- 不承诺与上游 `zai-org/ZCode` 的线格式一致；这里的真源是本仓库的 TS 实现。
