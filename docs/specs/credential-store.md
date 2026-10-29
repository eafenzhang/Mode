# Spec：凭据文件层（`crates/credential-store`）

状态：2026-10-29 实现（W3-4）。行为真源：

- `packages/services/src/credential/credentialService.ts`（读写、损坏隔离、锁范围）
- `packages/services/src/credential/providers/credentialCipherProvider.ts`（密文格式与密钥派生）
- `packages/shared/src/node/privateFilePersistence.ts`（私有原子写、坏文件备份、跨进程锁）

## 路径

```
dataRoot   = {dataBaseDir}/.mode        （壳侧 resolve_data_root 的返回值）
configDir  = {dataRoot}/v2
file       = {configDir}/credentials.json
```

`CredentialStore::new(data_root)` 自己拼 `v2`，与 `conversation-mirror` 的约定一致；
数据根的自定义指针、`MODE_DATA_BASE_DIR` 隔离由壳侧解析，本层不重复实现。

## 文件格式

```json
{
  "key": "enc:v1:<base64url(iv)>.<base64url(authTag)>.<base64url(ciphertext)>"
}
```

对象键值都必须是字符串（对应 `credentialRecordSchema = z.record(z.string(), z.string())`）。
**对象根必须是 JSON object**：数组、`null`、标量都按损坏处理。

## 密文（必须与 TS 逐字节兼容）

| 项 | 取值 |
| --- | --- |
| 算法 | AES-256-GCM |
| 密钥 | `sha256(secret)`，32 字节 |
| IV | 12 字节随机 |
| AuthTag | 16 字节，单独存放 |
| 编码 | base64url（**无 padding**），三段用 `.` 分隔 |
| 前缀 | `enc:v1:` |

`secret` 取值顺序与 TS 完全一致：

1. 环境变量 `MODE_CREDENTIAL_SECRET` 非空 → 直接作为 secret。
2. 否则 `mode-credential-fallback:{platform}:{homedir}:{username}`：
   - `platform`：`win32` / `darwin` / `linux`（`std::env::consts::OS` 的 `windows`/`macos`
     必须映射成 Node 的 `process.platform` 值）；
   - `homedir`：Windows `USERPROFILE` → `HOMEDRIVE`+`HOMEPATH`；Unix `HOME`；
   - `username`：Windows `GetUserNameW`；Unix `getpwuid_r(geteuid())`；取不到时 `"unknown"`。

**这条派生链不能各写各的**：只要 secret 与 Node 差一个字节，升级后所有旧凭据都会解不开。
`crates/credential-store` 的测试把平台映射与 fallback 拼接固定成黄金串。

解密行为：

- 不以 `enc:v1:` 开头 → 原样返回（兼容历史明文）。
- 三段结构、IV 长度、AuthTag 长度任一不合法 → 解密错误。
- GCM 校验失败 → 解密错误（不做明文回退）。

解密错误对应 TS 的 `Object.assign(new Error(...), { code })`：

```
message = "凭据解密失败：" + reason
code    = "MODE_CREDENTIAL_DECRYPT_FAILED"
```

## 读取

| 情况 | 结果 |
| --- | --- |
| 文件不存在（`ENOENT`） | 空表 |
| 其它 IO 错误 | `Read` 错误（消息含路径） |
| JSON/schema 非法 | 备份损坏文件后返回 `Corrupt` 错误 |

**损坏文件禁止自动覆盖**：把损坏 JSON 当空表继续 `save` 会清空其它登录凭据。备份路径
`credentials.json.corrupt-<sha256 前 24 位>.bak`，以 `create_new` + `0600` 排他创建
（同一份损坏内容重复读取只会留下一个备份）。

`list(prefix)` 只读键名、不解密：单条旧密文解密失败不能连坐展示性枚举；返回按键名
**升序**排序（与 TS `Object.keys(...).sort()` 一致）。

## 写入

`save` / `delete` 都是整文件 read-modify-write：

1. 建父目录；
2. 取 `credentials.json.lock` 跨进程锁（与 TS `withFileLock` 同一套目录锁）；
3. 锁内读取（错误语义同上）；
4. 锁内原子写：同目录临时文件 `0600` → rename（`EPERM/EBUSY/EACCES` 退避重试）；
5. 释放锁。

锁**必须覆盖读与写**：Desktop Host 与 CLI 是独立进程，进程内排队挡不住丢更新。
写回内容是 `JSON.stringify(data, null, 2) + "\n"`（两空格缩进、尾随换行）。

## 已知差异

| 差异 | 影响 |
| --- | --- |
| 对象键按字典序写回（`serde_json` 默认 Map） | 语义相同，文件字节顺序可能与 Node 不同 |
| 键校验错误文案 | TS 用 zod 文案；Rust 用固定英文消息。两者都不是接口契约 |
| `save`/`delete` 的 Host 私有 `onDidMutate` 通知 | 当前仓库没有调用方传入该回调（`onProviderProvisioningSourceChanged` 未接线），壳内不转发；接回该通知时需要同时补桥接 |
| secret 在 store 构造时读取 | 与 TS 在 `createCredentialCipherProvider()` 时读取一致；运行期改环境变量不生效 |

## 验收

`crates/credential-store/tests/credential_store.rs`：

- 不存在的文件 → 空表；`load` 缺失键 → `None`
- `save` → `load` 往返（含中文、emoji、空字符串）
- 写入文件形态（两空格缩进 + 尾随换行、Unix 上权限 `0600`）
- `delete` 只删目标键；`list` 按前缀过滤并升序
- `list` 不因某条密文损坏而失败
- 损坏 JSON → `Corrupt` + 唯一备份文件（内容保留、权限 `0600`）
- 再次读取同一损坏文件不产生第二个备份
- 明文值原样返回
- 非法密文（缺段、IV/tag 长度错、GCM 校验失败）→ 解密错误码
- 锁竞争下两个 store 的并发 `save` 不丢键（多线程）
- fallback secret 黄金串（平台映射 + 拼接顺序）

`crates/credential-store/tests/cipher_parity.rs`（`fixtures/credential-cipher.json`，由
`pnpm credential:fixtures` 用**真实 TS provider** 生成）：逐条解密 TS 产出的密文，断言
明文相等；并断言 Rust 加密结果满足同一结构。加密方向不做字节对拍（IV 随机），
跨语言兼容由「Rust 解 TS 密文」与「Rust 自往返」共同保证。
