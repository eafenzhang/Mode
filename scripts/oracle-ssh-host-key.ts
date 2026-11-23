// 对拍脚本：直接加载 TS 真源，产出 SSH host key 的期望值，供 Rust 侧断言使用。
// 运行：pnpm oracle:ssh-host-key
//
// 为什么保留这个脚本：`crates/workspace-identity` 的 `build_ssh_remote_host_key`
// 必须与 TS 的 `buildSshRemoteHostKey` **逐字节相等**——共享键一旦漂移，同一主机
// 会被当成两个远端，SSH 连接复用静默失效。这个脚本是那条断言的期望值来源。
import { buildSshRemoteHostKey } from "../packages/shared/src/remoteSshHostKey.ts";

interface SshTarget {
  kind: "ssh";
  host: string;
  port?: number;
  username: string;
  password?: string;
  privateKeyPath?: string;
}

interface OracleCase {
  label: string;
  target: SshTarget;
}

const cases: OracleCase[] = [
  {
    label: "private-key with windows drive path",
    target: {
      kind: "ssh",
      host: " Build-01 ",
      port: 2222,
      username: " dev ",
      privateKeyPath: "c:\\\\keys\\\\..\\\\id_ed25519",
    },
  },
  {
    label: "password (no port, defaults 22)",
    target: { kind: "ssh", host: "10.0.0.9", username: "root", password: "s3cret" },
  },
  {
    label: "agent (nothing)",
    target: { kind: "ssh", host: "10.0.0.9", username: "root" },
  },
  {
    label: "private key escapes root must not pop",
    target: {
      kind: "ssh",
      host: "h",
      username: "u",
      privateKeyPath: "/a/../../etc/ssh/id",
    },
  },
  {
    label: "unc share",
    target: {
      kind: "ssh",
      host: "h",
      username: "u",
      privateKeyPath: "//server/share/a/../b/id",
    },
  },
  {
    label: "tilde relative keeps parent",
    target: { kind: "ssh", host: "h", username: "u", privateKeyPath: "~/a/../../b" },
  },
  {
    label: "empty privateKeyPath falls back to agent",
    target: { kind: "ssh", host: "h", username: "u", privateKeyPath: "   " },
  },
  {
    label: "password and privateKey both -> private-key wins",
    target: {
      kind: "ssh",
      host: "h",
      username: "u",
      password: "x",
      privateKeyPath: "/k/id",
    },
  },
];

for (const testCase of cases) {
  console.log(JSON.stringify({ label: testCase.label, key: buildSshRemoteHostKey(testCase.target) }));
}
