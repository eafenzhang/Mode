/**
 * 浏览器 DSHF 分片：与 Node `FragmentReassembler` 同协议。
 * 若浏览器侧缺失这一层，Host → 浏览器 >48KB 的响应会被当成普通业务帧丢弃。
 */
import assert from "node:assert/strict";
import test from "node:test";

import {
  fragmentWebMessage,
  isWebFragment,
  WebFragmentReassembler,
} from "../src/web/fragments.js";

function plaintext(size: number): Uint8Array {
  const bytes = new Uint8Array(size);
  for (let index = 0; index < size; index += 1) bytes[index] = index % 251;
  return bytes;
}

test("大消息分片后按序重组，字节完全一致", () => {
  const source = plaintext(120_000);
  const fragments = fragmentWebMessage(7, source);
  assert.equal(fragments.length, 3);
  assert.equal(isWebFragment(fragments[0]!), true);
  assert.equal(isWebFragment(new TextEncoder().encode('{"v":1}')), false);

  const reassembler = new WebFragmentReassembler();
  let result: Uint8Array | undefined;
  for (const fragment of fragments) {
    const complete = reassembler.push(fragment);
    if (complete) result = complete.plaintext;
  }
  assert.ok(result);
  assert.equal(result!.length, source.length);
  assert.deepEqual(Buffer.from(result!), Buffer.from(source));
});

test("小消息单个分片也能重组", () => {
  const source = new TextEncoder().encode("hello");
  const fragments = fragmentWebMessage(0, source);
  assert.equal(fragments.length, 1);
  const complete = new WebFragmentReassembler().push(fragments[0]!);
  assert.deepEqual(complete?.plaintext, source);
});

test("重复/乱序/缺片一律 fail closed", () => {
  const fragments = fragmentWebMessage(9, plaintext(120_000));
  const duplicate = new WebFragmentReassembler();
  duplicate.push(fragments[0]!);
  assert.throws(() => duplicate.push(fragments[0]!));

  const outOfOrder = new WebFragmentReassembler();
  assert.throws(() => outOfOrder.push(fragments[2]!));
});
