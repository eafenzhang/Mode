//! OrcaRouter 服务（分阶段迁移）。
//!
//! 规范见 `docs/specs/orcarouter-service.md`。当前已迁：凭据存储（阶段 1）与
//! PKCE 连接控制器（阶段 2）；目录/overlay/频道在后续阶段。

pub mod catalog;
pub mod connect;
pub mod credential;
pub mod origins;
pub mod overlay;

pub use catalog::{
    filter_models, parse_catalog, parse_model_record, OrcaCapability, OrcaCatalogInput,
    OrcaCatalogOptions, OrcaCatalogService, OrcaCatalogSource, OrcaCatalogView, OrcaModelRecord,
    OrcaModelRecordView,
};
pub use connect::{
    create_pkce_material, safe_state_equals, OrcaConnectController, OrcaConnectError,
    OrcaConnectErrorKind, OrcaConnectOptions, OrcaConnectPhase, OrcaConnectState,
};
pub use credential::{
    looks_like_orca_api_key, mask_orca_secret, OrcaCredential, OrcaCredentialSource,
    OrcaCredentialStatus, OrcaCredentialStore, ORCAROUTER_CREDENTIAL_KEY,
};
pub use origins::{
    build_orca_authorize_url, build_orca_models_url, build_orca_v1_base, normalize_orca_origin,
    resolve_orca_origins, OrcaAuthorizeUrlInput, OrcaOrigins, ORCAROUTER_CODE_CHALLENGE_METHOD,
    ORCAROUTER_DEFAULT_API_BASE, ORCAROUTER_DEFAULT_AUTH_BASE, ORCAROUTER_EXCHANGE_PATH,
    ORCAROUTER_MODELS_PATH, ORCAROUTER_REQUESTED_SCOPE,
};
pub use overlay::{OrcaProviderCredentialBinding, ORCAROUTER_PROVIDER_TEMPLATE_ID};
