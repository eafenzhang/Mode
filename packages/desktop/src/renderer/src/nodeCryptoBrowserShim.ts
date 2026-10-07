/**
 * 渲染端 node:crypto 浏览器 shim。
 *
 * 修复原因：packages/services 以 TypeScript 源码形式进入 renderer 的 vite 模块图，
 * 其中 orcarouter/connect.ts 顶层 `import { createHash } from "node:crypto"`。
 * 在浏览器环境里 vite 会把 node:crypto 外部化成"访问即抛错"的 stub，
 * 模块求值阶段解构导出就直接 Uncaught Error，整个 React 应用无法挂载（黑屏）。
 *
 * 该 shim 让模块求值成功；真被调用时按能力区分：
 * - randomUUID / randomBytes / timingSafeEqual 用 WebCrypto 给出正确实现；
 * - createHash 需要同步流式哈希（WebCrypto digest 是异步的），渲染端没有合法调用
 *   场景（PKCE/OAuth 均发生在 Host 进程），调用时抛出带指引的错误。
 */

const nodeCryptoNotAvailable = (api: string): Error =>
  new Error(
    `[renderer] node:crypto.${api} 不可用：该能力仅存在于 Host/主进程。` +
      `渲染端代码不应执行到这里，请检查是否有仅限 Node 的模块被引入了浏览器模块图。`,
  );

export function randomUUID(): string {
  return globalThis.crypto.randomUUID();
}

export function randomBytes(size: number): Uint8Array {
  const bytes = new Uint8Array(size);
  globalThis.crypto.getRandomValues(bytes);
  return bytes;
}

export function timingSafeEqual(a: Uint8Array, b: Uint8Array): boolean {
  if (a.length !== b.length) {
    return false;
  }
  let mismatch = 0;
  for (let index = 0; index < a.length; index += 1) {
    mismatch |= a[index]! ^ b[index]!;
  }
  return mismatch === 0;
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any -- 对齐 Node createHash 的宽松调用面
export function createHash(_algorithm: string): any {
  throw nodeCryptoNotAvailable("createHash");
}

export default {
  get createHash() {
    return createHash;
  },
  get randomBytes() {
    return randomBytes;
  },
  get randomUUID() {
    return randomUUID;
  },
  get timingSafeEqual() {
    return timingSafeEqual;
  },
};
