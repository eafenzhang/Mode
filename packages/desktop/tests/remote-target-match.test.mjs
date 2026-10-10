import { test } from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { transpileModule, ModuleKind } from "typescript";

// LAN 远程工作区 IM 机器人绑定回归（docs/specs/im-bot-remote-workspace-binding.md）：
// main 侧按 target 匹配路由的唯一口径是 isSameRemoteTarget（连接状态查询、
// Bot runtime port attachment、断线重连复用共用它）。它曾缺 lan 分支，
// 两个完全相同的 lan target 从 switch 掉出返回 undefined → 判否 →
// bindBotToTask 的流观看建立必抛「未找到可供 Bot attachment 的远程 logical session」，
// 而绑定菜单没有 catch——表现为「远程工作区无法绑定 IM 机器人」且无任何提示。
// remoteTargetMatch.ts 不依赖 electron，用 transpile + new Function 直接加载真实源码。
const root = new URL("../../../", import.meta.url);

async function loadTargetMatchModule() {
  const source = await readFile(
    new URL("packages/desktop/src/main/remoteTargetMatch.ts", root),
    "utf8",
  );
  const exports = {};
  new Function(
    "require",
    "exports",
    transpileModule(source, {
      compilerOptions: { module: ModuleKind.CommonJS },
    }).outputText,
  )((name) => {
    throw new Error(`remoteTargetMatch.ts 不应有运行时 import（出现 ${name}）`);
  }, exports);
  return exports;
}

const { isSameRemoteTarget } = await loadTargetMatchModule();

test("lan target 判等：同 host+port 视为同一 target（token 不参与比较）", () => {
  const base = { kind: "lan", host: "192.168.1.10", port: 45879 };
  // settings 快照不带 token，连接期 live target 带 token——必须判等，否则绑定路由永远找不到 session
  assert.equal(isSameRemoteTarget({ ...base }, { ...base, token: "secret" }), true);
  // host 大小写与 identity 口径一致（小写比较）
  assert.equal(
    isSameRemoteTarget(
      { kind: "lan", host: "Host.Lan", port: 1 },
      { kind: "lan", host: "host.lan", port: 1 },
    ),
    true,
  );
  assert.equal(isSameRemoteTarget({ ...base }, { ...base, port: 45880 }), false);
  assert.equal(isSameRemoteTarget({ ...base }, { ...base, host: "192.168.1.11" }), false);
  // 不同 kind 一律判否
  assert.equal(isSameRemoteTarget({ ...base }, { kind: "docker", container: "x" }), false);
});

test("ssh/wsl/docker 判等口径保持不变", () => {
  const ssh = { kind: "ssh", host: "Example.COM", port: 22, username: "u" };
  assert.equal(isSameRemoteTarget({ ...ssh }, { ...ssh, password: "p" }), true);
  assert.equal(isSameRemoteTarget({ ...ssh }, { ...ssh, port: 2222 }), false);
  assert.equal(
    isSameRemoteTarget({ ...ssh, port: undefined }, { ...ssh, port: 22 }),
    true,
    "ssh port 缺省按 22 归一",
  );
  assert.equal(
    isSameRemoteTarget({ kind: "wsl" }, { kind: "wsl" }),
    true,
    "wsl distro/user 缺省按 default 归一",
  );
  assert.equal(isSameRemoteTarget({ kind: "wsl", distro: "Ubuntu" }, { kind: "wsl" }), false);
  assert.equal(
    isSameRemoteTarget({ kind: "docker", container: "a" }, { kind: "docker", container: "b" }),
    false,
  );
});
