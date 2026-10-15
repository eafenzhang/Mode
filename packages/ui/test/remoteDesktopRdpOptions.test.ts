import assert from "node:assert/strict";
import test from "node:test";

// 每设备连接选项的前端纯逻辑（docs/specs/remote-desktop-system-rdp.md 产品规则 9）：
// 缺省值 = 窗口模式、多显示器关、剪贴板开、音频带到本机、端口 3389；
// 脏数据（未知音频 / 越界端口）回退缺省，确认时用户名去空白且空串不写。
const { resolveRdpOptions, toRdpConnectionOptions, RDP_OPTION_DEFAULTS } = await import(
  "../src/remote-desktop/rdpOptions.js"
);

test("缺省值：窗口模式、剪贴板开、音频带到本机、端口 3389", () => {
  assert.deepEqual(resolveRdpOptions(undefined), {
    fullscreen: false,
    multimon: false,
    clipboard: true,
    audio: "local",
    username: "",
    port: 3389,
  });
  assert.deepEqual(resolveRdpOptions(null), RDP_OPTION_DEFAULTS);
  assert.deepEqual(resolveRdpOptions({}), RDP_OPTION_DEFAULTS);
});

test("部分字段覆盖：只改指定项，其余取缺省", () => {
  assert.deepEqual(resolveRdpOptions({ fullscreen: true, port: 3390 }), {
    fullscreen: true,
    multimon: false,
    clipboard: true,
    audio: "local",
    username: "",
    port: 3390,
  });
  assert.deepEqual(resolveRdpOptions({ clipboard: false, audio: "disabled" }), {
    fullscreen: false,
    multimon: false,
    clipboard: false,
    audio: "disabled",
    username: "",
    port: 3389,
  });
});

test("脏数据回退：未知音频按 local、越界端口按 3389", () => {
  assert.equal(resolveRdpOptions({ audio: "bogus" as never }).audio, "local");
  assert.equal(resolveRdpOptions({ port: 0 }).port, 3389);
  assert.equal(resolveRdpOptions({ port: 70000 }).port, 3389);
  assert.equal(resolveRdpOptions({ port: 1.5 }).port, 3389);
});

test("提交收敛：用户名去首尾空白，空串不写键", () => {
  assert.deepEqual(toRdpConnectionOptions({ ...RDP_OPTION_DEFAULTS, username: "  bob  " }), {
    fullscreen: false,
    multimon: false,
    clipboard: true,
    audio: "local",
    username: "bob",
    port: 3389,
  });
  const noUsername = toRdpConnectionOptions({ ...RDP_OPTION_DEFAULTS, username: "   " });
  assert.ok(!("username" in noUsername), "空用户名不应写入连接选项");
});
