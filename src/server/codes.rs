//! Stable runtime error-code strings shared by the HTTP envelopes.

pub const AUTH_SERVICE_TOKEN_INVALID: &str = "auth.service_token_invalid";
pub const IDENTITY_HEADER_MISSING: &str = "identity.header_missing";
/// A 401 the consumer may resolve with one token refresh, then a single resend.
pub const IDENTITY_TOKEN_REFRESHABLE: &str = "identity.token_refreshable";
/// A 401 no refresh can resolve: re-login, or an administrator has to act. Consumers must
/// not enter the refresh path on this one, or a deactivated account loops forever.
pub const IDENTITY_TOKEN_TERMINAL: &str = "identity.token_terminal";
pub const IDENTITY_UPSTREAM_UNAVAILABLE: &str = "identity.upstream_unavailable";
/// The upstream reported conflicting credentials on our request — a runtime
/// request-construction bug (we only ever send Bearer), never a property of the token.
pub const IDENTITY_UPSTREAM_CONFLICT: &str = "identity.upstream_conflict";
pub const AUTHZ_INSUFFICIENT: &str = "authz.insufficient";
pub const RATE_LIMIT_GLOBAL: &str = "rate_limit.global";
pub const RATE_LIMIT_ACTOR: &str = "rate_limit.actor";
pub const REQUEST_INVALID: &str = "request.invalid";
pub const UPSTREAM_ERROR: &str = "upstream.error";
/// The report pipeline ended without a renderable `report.data` (the fetched material lacked the
/// monthly or station figures a report needs, or the model never produced a valid payload).
pub const REPORT_DATA_UNAVAILABLE: &str = "report.data_unavailable";
pub const SERVER_UNAVAILABLE: &str = "server.unavailable";
pub const SERVER_INTERNAL: &str = "server.internal";
