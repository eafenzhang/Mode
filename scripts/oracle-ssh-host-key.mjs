// 对拍脚本：用 Node 24 的 type-stripping 直接加载 TS 真源，产出 SSH host key 与
// WSL/connection-key 的期望值，供 Rust 侧断言使用。
// 运行：node scripts/oracle-ssh-host-key.mjs
import { buildSshRemoteHostKey } from "../packages/shared/src/remoteSshHostKey.ts";

const cases = [
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
