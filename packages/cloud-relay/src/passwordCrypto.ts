/**
 * 个人账号密码哈希（docs/specs/cloud-personal-accounts.md）。
 *
 * scrypt（node:crypto 内置，无原生依赖）：N=2^15, r=8, p=1，每用户随机 salt。
 * 存储格式 `scrypt$N$r$p$<salt b64>$<hash b64>`，verify 时按存储参数回放，
 * 并对参数设上限防止伪造超参记录造成内存耗尽。
 */
import { randomBytes, scrypt as scryptCallback, timingSafeEqual } from "node:crypto";
import { promisify } from "node:util";

const scrypt = promisify(scryptCallback) as (
  password: string | Buffer,
  salt: string | Buffer,
  keylen: number,
  options: { N: number; r: number; p: number; maxmem?: number },
) => Promise<Buffer>;

const SCRYPT_N = 32_768;
const SCRYPT_R = 8;
const SCRYPT_P = 1;
const KEY_LENGTH = 32;
const SALT_LENGTH = 16;
/**
 * Node 默认 maxmem=32MB，而 128*N*r = 32MiB（N=2^15, r=8）正好压线报
 * "memory limit exceeded"，必须显式放宽。
 */
const SCRYPT_MAXMEM = 128 * 1024 * 1024;
/** verify 时的参数上限（当前参数的 32 倍余量），防御恶意构造的存储串。 */
const MAX_N = 2 ** 20;
const MAX_R = 64;

export async function hashPassword(password: string): Promise<string> {
  const salt = randomBytes(SALT_LENGTH);
  const derived = await scrypt(password.normalize("NFKC"), salt, KEY_LENGTH, {
    N: SCRYPT_N,
    r: SCRYPT_R,
    p: SCRYPT_P,
    maxmem: SCRYPT_MAXMEM,
  });
  return `scrypt$${SCRYPT_N}$${SCRYPT_R}$${SCRYPT_P}$${salt.toString("base64")}$${derived.toString("base64")}`;
}

export async function verifyPassword(password: string, stored: string): Promise<boolean> {
  const parts = stored.split("$");
  if (parts.length !== 6 || parts[0] !== "scrypt") {
    return false;
  }
  const n = Number(parts[1]);
  const r = Number(parts[2]);
  const p = Number(parts[3]);
  if (
    !Number.isInteger(n) ||
    !Number.isInteger(r) ||
    !Number.isInteger(p) ||
    n < 2 ||
    n > MAX_N ||
    r < 1 ||
    r > MAX_R ||
    p < 1 ||
    p > 8
  ) {
    return false;
  }
  let salt: Buffer;
  let expected: Buffer;
  try {
    salt = Buffer.from(parts[4]!, "base64");
    expected = Buffer.from(parts[5]!, "base64");
  } catch {
    return false;
  }
  if (salt.length === 0 || expected.length === 0 || expected.length > 128) {
    return false;
  }
  try {
    const actual = await scrypt(password.normalize("NFKC"), salt, expected.length, {
      N: n,
      r,
      p,
      maxmem: SCRYPT_MAXMEM,
    });
    return actual.length === expected.length && timingSafeEqual(actual, expected);
  } catch {
    // 参数在数值范围内但内存超限（恶意构造）：按验证失败处理，不向上抛。
    return false;
  }
}
