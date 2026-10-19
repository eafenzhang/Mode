/**
 * 服务适配器的内部成员隔离。
 *
 * `ProxyChannel.fromService` 用 `for...in` 枚举对象上的全部方法，把每个函数都映射成 RPC
 * command。适配器除了契约方法之外还有 Host 内部的生命周期方法（`dispose`、`getActiveServer`、
 * `handleMessage`、`disposeHost`），它们绝不能出现在 renderer 的 RPC 面上。
 *
 * 做法：把这些成员的属性改成不可枚举。`for...in` 会跳过它们，Host 内部仍可直接调用，
 * 类型也不受影响。
 */
export function hideInternalServiceMembers<T extends object>(
  service: T,
  names: readonly (keyof T & string)[],
): T {
  for (const name of names) {
    const descriptor = Object.getOwnPropertyDescriptor(service, name);
    if (!descriptor) {
      continue;
    }
    Object.defineProperty(service, name, { ...descriptor, enumerable: false });
  }
  return service;
}
