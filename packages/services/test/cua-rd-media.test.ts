import assert from "node:assert/strict";
import test from "node:test";
import { BrokerError, CuaHelperError } from "@mode/cua/broker";

// 远程桌面（LAN）Task 8：媒体面注入端口（一次 rd_media_* 调用；不重试、错误码按既有收敛口径）。
// socket 解析与 broker 客户端均按依赖注入替换（对照 cua-raw-input.test.ts 的目录既有 mock 风格）。
const { createCuaRdMediaController } = await import("../src/cua-permission-broker/cuaRdMedia.js");

const FAKE_SOCKET = "\\\\.\\pipe\\mode-cua-rd-media-test";

test("媒体调用：method/params/timeout 原样透传，broker 结果原样 resolve", async () => {
  const captured: Array<Record<string, unknown>> = [];
  const controller = createCuaRdMediaController({
    resolveSocketPath: () => FAKE_SOCKET,
    callBrokerMethod: async (args) => {
      captured.push(args as unknown as Record<string, unknown>);
      // 软件回退（2026-10-15）：被控 host 从 rd_media_start 的 result 读编码器来源，
      // 故本层必须原样透传 broker 结果（不得吞成 void）。
      return { ok: true, encoder: "software" };
    },
  });
  const params = { rect: { x: 0, y: 0, width: 100, height: 100 } };
  assert.deepEqual(await controller("rd_media_start", params), {
    ok: true,
    encoder: "software",
  });
  // 超时口径：媒体面单次调用 15s（napi 建流 ~10s + 余量），switch/stop 同预算。
  assert.deepEqual(captured, [
    { socketPath: FAKE_SOCKET, method: "rd_media_start", params, timeoutMs: 15_000 },
  ]);
});

test("媒体不可达：解析点返回 null → helper_unavailable，且不发起调用", async () => {
  let calls = 0;
  const controller = createCuaRdMediaController({
    resolveSocketPath: () => null,
    callBrokerMethod: async () => {
      calls += 1;
    },
  });
  await assert.rejects(
    controller("rd_media_stop", {}),
    (error) => error instanceof CuaHelperError && error.code === "helper_unavailable",
  );
  assert.equal(calls, 0);
});

test("媒体不可达：解析点抛错同样收敛为 helper_unavailable，不冒裸错误", async () => {
  const controller = createCuaRdMediaController({
    resolveSocketPath: () => {
      throw new Error("resolver exploded");
    },
    callBrokerMethod: async () => ({}),
  });
  await assert.rejects(
    controller("rd_media_switch", { rect: { x: 1, y: 1, width: 2, height: 2 } }),
    (error) =>
      error instanceof CuaHelperError &&
      error.code === "helper_unavailable" &&
      // 与 cuaRawInput 的 toInjectError 同口径：稳定码收敛，但保留原始 message 供 warn 日志定位。
      error.message === "resolver exploded",
  );
});

test("媒体不可达：连接层错误（stale_socket / timeout / unavailable / broker_unavailable）收敛为 helper_unavailable", async () => {
  for (const code of ["stale_socket", "timeout", "unavailable", "broker_unavailable"]) {
    const controller = createCuaRdMediaController({
      resolveSocketPath: () => FAKE_SOCKET,
      callBrokerMethod: async () => {
        throw new BrokerError(`simulated ${code}`, { code });
      },
    });
    await assert.rejects(
      controller("rd_media_start", {}),
      (error) => error instanceof CuaHelperError && error.code === "helper_unavailable",
      code,
    );
  }
});

test("媒体失败：helper 明确应答的协议错误保留原码", async () => {
  for (const code of ["invalid_request", "method_not_found"]) {
    const controller = createCuaRdMediaController({
      resolveSocketPath: () => FAKE_SOCKET,
      callBrokerMethod: async () => {
        throw new BrokerError(`helper rejected with ${code}`, { code });
      },
    });
    await assert.rejects(
      controller("rd_media_switch", {}),
      (error) => error instanceof CuaHelperError && error.code === code,
      code,
    );
  }
});
