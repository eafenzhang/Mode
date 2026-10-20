import { networkInterfaces } from "node:os";

/** 列出本机可在局域网访问的 IPv4 地址（含端口）；跳过回环与内部地址。 */
export function listLanAddresses(port: number): string[] {
  const addresses: string[] = [];
  for (const infos of Object.values(networkInterfaces())) {
    for (const info of infos ?? []) {
      if (info.family !== "IPv4" || info.internal) {
        continue;
      }
      addresses.push(`${info.address}:${port}`);
    }
  }
  return addresses;
}
