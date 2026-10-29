//! 凭据文件层（W3-4）。
//!
//! 行为真源：
//! - `packages/services/src/credential/credentialService.ts`
//! - `packages/services/src/credential/providers/credentialCipherProvider.ts`
//! - `packages/shared/src/node/privateFilePersistence.ts`
//!
//! 规范见 `docs/specs/credential-store.md`。密文格式与密钥派生必须与 Node 兼容，
//! 否则壳内接管频道后旧凭据会全部解不开。

mod base64url;
mod cipher;
mod store;

pub use cipher::{
    fallback_secret, resolve_secret, Cipher, CipherError, CREDENTIAL_SECRET_ENV_KEY,
    ENCRYPTED_VALUE_PREFIX,
};
pub use store::{
    CorruptListener, CredentialError, CredentialStore, CREDENTIALS_FILE_NAME,
    CREDENTIAL_DECRYPT_ERROR_CODE, CREDENTIAL_DECRYPT_ERROR_PREFIX,
};
