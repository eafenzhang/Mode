/**
 * 配对 label 夹具：pairLanPeer 在 host 执行，未显式传 label 时必须带上本机 hostname
 * （对端「配对我的设备」列表按它显示设备名）；显式 label 仍然优先（验证脚本在用）。
 * 向导此前传 navigator.platform（Windows 上是 "Win32"），已在 UI 侧删除。
 * 失效模式（未实现时）：redeem 收到的 label 为 undefined → 断言失败。
 */
import { hostname } from "node:os";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { AddressInfo } from "node:net";

// MODE_DATA_BASE_DIR 在 services 模块加载时读取，必须先设再动态导入。
process.env.MODE_DATA_BASE_DIR = await mkdtemp(join(tmpdir(), "mode-pair-label-"));
const { createCredentialService } = await import("@mode/services/node");
const { createHttpServer } = await import("@mode/server");
const { pairLanPeer } = await import("../../src/host/lanRemoteAttach.js");

const capturedLabels: Array<string | undefined> = [];
const servicesStub = {
  getOptional: () => undefined,
  exposeOnChannelServer: () => undefined,
} as never;

const server = createHttpServer(servicesStub, 0, {
  serverId: "label-fixture",
  name: "label-fixture",
  lanPairing: {
    redeem: async (request: { label?: string }) => {
      capturedLabels.push(request.label);
      return { token: "fixture-token", serverId: "label-fixture-server" };
    },
  },
});

try {
  await new Promise<void>((resolve) => server.once("listening", () => resolve()));
  const port = (server.address() as AddressInfo).port;
  const credentials = createCredentialService();

  // 1) 不传 label（向导删除 navigator.platform 后的形态）→ host 兜底 hostname
  await pairLanPeer({ credentials, host: "127.0.0.1", port, code: "ABC123" });
  if (capturedLabels[0] !== hostname()) {
    throw new Error(`期望 label=hostname(${hostname()})，实际=${String(capturedLabels[0])}`);
  }

  // 2) 显式 label 仍然优先（验证脚本路径不回归）
  await pairLanPeer({
    credentials,
    host: "127.0.0.1",
    port,
    code: "ABC124",
    label: "verify-script",
  });
  if (capturedLabels[1] !== "verify-script") {
    throw new Error(`显式 label 未生效，实际=${String(capturedLabels[1])}`);
  }

  console.log("PAIR_LABEL_OK");
} finally {
  (server as unknown as { closeAllConnections?: () => void }).closeAllConnections?.();
  await new Promise<void>((resolve) => server.close(() => resolve()));
  await new Promise((resolve) => setTimeout(resolve, 100));
}
