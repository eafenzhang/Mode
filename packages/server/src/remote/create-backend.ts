import { readFile } from "node:fs/promises";
import { homedir } from "node:os";
import type { RemoteTarget } from "@mode/shared";
import type { IRemoteBackend } from "./backend.js";

export async function createRemoteBackend(target: RemoteTarget): Promise<IRemoteBackend> {
  switch (target.kind) {
    case "ssh": {
      const { SSHBackend } = await import("./ssh-backend.js");
      let privateKey: string | Buffer | undefined;
      if (target.privateKeyPath) {
        const keyPath = target.privateKeyPath.replace(/^~/, homedir());
        privateKey = await readFile(keyPath);
      }

      return new SSHBackend({
        host: target.host,
        port: target.port,
        username: target.username,
        password: target.password,
        privateKeyPath: target.privateKeyPath,
        privateKeyPassphrase: target.privateKeyPassphrase,
        privateKey,
      });
    }
    case "wsl": {
      const { WSLBackend } = await import("./wsl-backend.js");
      return new WSLBackend(target);
    }
    case "docker": {
      const { DockerBackend } = await import("./docker-backend.js");
      return new DockerBackend(target);
    }
    case "lan": {
      // 局域网对端是「已经在跑的实例」：客户端走 attachLanRemoteConnection
      // （HTTP /api/server-info + /ws/host），不做上传/启动 stdio server，因此没有 backend。
      throw new Error("LAN 目标没有 stdio backend；请使用局域网挂接路径");
    }
    case "v3": {
      // v3 目标同样是在线实例，但连接面在桌面 Host 的账号设备目录与 Noise 通道里；
      // 服务端没有设备私钥，无法替客户端建立后端。
      throw new Error("v3 远程工作区仅在桌面 Host 支持；服务端没有设备身份");
    }
  }
}
