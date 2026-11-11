/** 线协议/安全通道错误。code 是稳定机器码，retryable 只描述传输层可否重试。 */
export class RemoteError extends Error {
  readonly code: string;
  readonly retryable: boolean;

  constructor(code: string, message: string, options?: { retryable?: boolean; cause?: unknown }) {
    super(message, options?.cause === undefined ? undefined : { cause: options.cause });
    this.name = "RemoteError";
    this.code = code;
    this.retryable = options?.retryable ?? false;
  }
}

export function invalid(message: string, cause?: unknown): never {
  throw new RemoteError("invalid_message", message, { cause });
}

export function failClosed(code: string, message: string): never {
  throw new RemoteError(code, message);
}
