# @mode/remote-core

远程控制 v3 的设备身份、Noise 端到端加密、协议帧与 Host/Client 控制运行时。
规范：[docs/specs/remote-control-v3.md](../../docs/specs/remote-control-v3.md)。

## 目录

```text
src/domain/     协议帧、限额、关闭语义、DSHF 分片、descriptor 校验
src/adapters/   X25519 身份、Noise IK / transport（node:crypto）
src/app/        SecureSession、StreamMux、ControlLink / ConnectionRuntime
src/contract.ts 对外类型
src/index.ts    对外工厂
```

## 密码套件

```text
Noise_IK_25519_ChaChaPoly_SHA256
prologue = "MODE-REMOTE-V3" || version || connectionId || hostDeviceId || clientDeviceId
```

- 私钥只在设备本地；服务端只保存公钥。
- Noise transport 每方向独立 nonce；认证失败、重放、乱序、超限一律 fail closed。
- 业务帧 >48 KiB 先切 DSHF 分片，完整消息上限 4 MiB。

## 运行时

`ControlLink` 负责 Control WS 的 hello、连接授权、`secure.handshake` 路由、
Relay `attach/ready` 与密文收发。调用方注入 `connectSocket` 与 `fetchPeer`：

```ts
const link = new ControlLink({
  role: "client",
  serverUrl: "https://remote.example",
  deviceId,
  accessToken,
  identity,
  connectSocket,
  fetchPeer,
});
await link.start();
const connection = await link.connect(hostDeviceId);
connection.onFrame((frame) => { /* rpc.request / event / stream.* */ });
```

首期只实现 Relay 数据面；LAN direct / WebRTC 在 P2 接入前不会宣告对应 capability。

**生产接线必须提供 `verifyPeer`**：账号服务只决定 membership，本地 pinning 需要调用方
按 `serverOrigin + role + peerDeviceId → identityKey` 持久化并在公钥变化时拒绝连接。

## 测试

```sh
pnpm --filter @mode/remote-core test
```
