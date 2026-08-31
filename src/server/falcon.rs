//! Falcon permissions client and bounded cache.
//!
//! The integration guide documents the permissions endpoint's Bearer transport and successful
//! response shape, but not its error codes — the live endpoint nonetheless returns an
//! `error_code` in the 401 body, confirmed by probe. This module therefore reads that code to
//! separate a refreshable 401 from a terminal one, using a whitelist so an unknown or renamed
//! code stays terminal. The upstream code is an internal input only: it is never forwarded to
//! consumers, who see this service's own stable `code` instead.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use async_trait::async_trait;
use reqwest::StatusCode;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::config::IdentityConfig;

/// Default upper bound for the combined positive and negative permission cache.
pub const DEFAULT_CACHE_CAPACITY: usize = 1024;

/// A permission returned by Falcon's documented permissions array.
///
/// Unknown fields are **accepted**. Adding a field to a JSON payload is a routine,
/// backward-compatible upstream change — and this endpoint has already shipped behavior the
/// integration guide never documented, so it will happen. Rejecting unknown fields would turn
/// one additive field into a parse failure, a negative-cached `Unavailable`, and a `503` for
/// every user on both prompt routes until a code change ships. Fail-closed means an unknown
/// *401 code* is terminal, not that an unknown *200 field* is an outage.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct PermissionItem {
    pub code: String,
    pub name: String,
    pub category: String,
    #[serde(default)]
    pub page_path: Option<String>,
    pub can_read: bool,
    pub can_write: bool,
}

/// A role returned by Falcon's documented roles array.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct FalconRole {
    pub id: i64,
    pub code: String,
    pub name: String,
    #[serde(default)]
    pub default_pages: Vec<String>,
}

/// The exact successful permissions response shape documented by Falcon.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct FalconPermissionsResponse {
    pub user_id: i64,
    #[serde(default)]
    pub roles: Vec<FalconRole>,
    pub permissions: Vec<PermissionItem>,
}

/// The reduced permission set used by the runtime after parsing Falcon's response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Permissions {
    pub user_id: i64,
    pub codes: std::collections::HashSet<String>,
}

impl FalconPermissionsResponse {
    pub fn into_permissions(self) -> Permissions {
        Permissions {
            user_id: self.user_id,
            codes: self
                .permissions
                .into_iter()
                .filter(|permission| permission.can_read)
                .map(|permission| permission.code)
                .collect(),
        }
    }
}

/// Failures the runtime distinguishes when a permissions lookup does not succeed.
///
/// The 401 split is the load-bearing part. The live endpoint returns an `error_code` in the
/// body, and only one of its values can be fixed by refreshing the token; the rest describe
/// states a new token will not change (revoked, deactivated, unknown user, wrong platform).
/// Consumers branch on this, so collapsing the two would let a deactivated account drive an
/// endless refresh loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionsFailure {
    /// A 401 whose `error_code` says a token refresh may resolve it.
    UnauthorizedRefreshable,
    /// A 401 that refreshing cannot resolve. Also the default for an unrecognised or
    /// absent `error_code`, because the codes are not covered by the integration guide.
    UnauthorizedTerminal,
    /// The endpoint could not provide a usable permissions response.
    Unavailable,
}

pub type PermissionsResult = std::result::Result<Permissions, PermissionsFailure>;

/// Provider seam used by the identity middleware and by in-process tests.
#[async_trait]
pub trait PermissionsProvider: Send + Sync {
    async fn permissions(&self, token: &str) -> PermissionsResult;
}

/// Parse the documented successful response. This is intentionally a separate seam so the wire
/// shape can be tested without making a network request.
pub fn parse_permissions_response(value: serde_json::Value) -> Result<Permissions> {
    let response: FalconPermissionsResponse = serde_json::from_value(value)?;
    Ok(response.into_permissions())
}

/// Classify a non-200 status. A 401 needs its body too, so callers route it through
/// [`classify_unauthorized`]; every other status is simply unusable.
pub fn classify_status(status: StatusCode) -> PermissionsFailure {
    if status == StatusCode::UNAUTHORIZED {
        PermissionsFailure::UnauthorizedTerminal
    } else {
        PermissionsFailure::Unavailable
    }
}

/// The one `error_code` a token refresh can resolve.
///
/// Verified live on the dev endpoint alongside `auth.missing_token`; the remaining values
/// were relayed by the backend team. They appear in **no** version of the integration guide,
/// so this is deliberately a whitelist rather than a list of terminal codes: an unknown or
/// renamed code stays terminal, and the worst case is one unnecessary re-login instead of a
/// refresh loop.
const REFRESHABLE_ERROR_CODE: &str = "auth.token_invalid";

/// Classify a 401 from its `error_code`, defaulting to terminal.
pub fn classify_unauthorized(error_code: Option<&str>) -> PermissionsFailure {
    if error_code == Some(REFRESHABLE_ERROR_CODE) {
        PermissionsFailure::UnauthorizedRefreshable
    } else {
        PermissionsFailure::UnauthorizedTerminal
    }
}

/// Read `error_code` out of the endpoint's error body, e.g.
/// `{"detail": "未登入", "error_code": "auth.missing_token"}`.
pub fn error_code_of(body: &serde_json::Value) -> Option<String> {
    body.get("error_code")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

#[derive(Debug, Clone)]
enum CacheValue {
    Positive(Permissions),
    Negative(PermissionsFailure),
}

#[derive(Debug, Clone)]
struct CacheEntry {
    value: CacheValue,
    expires_at: Instant,
}

#[derive(Debug)]
struct PermissionCache {
    entries: HashMap<String, CacheEntry>,
    lru: VecDeque<String>,
    capacity: usize,
}

impl PermissionCache {
    fn new(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            lru: VecDeque::new(),
            capacity: capacity.max(1),
        }
    }

    fn get(&mut self, key: &str, now: Instant) -> Option<CacheValue> {
        let entry = self.entries.get(key)?;
        if entry.expires_at <= now {
            self.entries.remove(key);
            self.remove_lru(key);
            return None;
        }
        let value = entry.value.clone();
        self.touch(key);
        Some(value)
    }

    fn insert(&mut self, key: String, value: CacheValue, expires_at: Instant) {
        self.entries
            .insert(key.clone(), CacheEntry { value, expires_at });
        self.touch(&key);
        while self.entries.len() > self.capacity {
            let Some(oldest) = self.lru.pop_front() else {
                break;
            };
            self.entries.remove(&oldest);
        }
    }

    fn touch(&mut self, key: &str) {
        self.remove_lru(key);
        self.lru.push_back(key.to_string());
    }

    fn remove_lru(&mut self, key: &str) {
        self.lru.retain(|candidate| candidate != key);
    }
}

/// HTTP implementation of the permissions provider with positive and negative caching.
pub struct FalconPermissionsClient {
    client: reqwest::Client,
    endpoint: String,
    positive_ttl: Duration,
    negative_ttl: Duration,
    cache: Arc<Mutex<PermissionCache>>,
}

impl FalconPermissionsClient {
    pub fn from_config(config: &IdentityConfig) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(config.request_timeout_ms.get()))
            .build()?;
        Ok(Self::new_with_client(
            client,
            &config.base_url,
            Duration::from_millis(config.positive_ttl_ms.get()),
            Duration::from_millis(config.negative_ttl_ms.get()),
            DEFAULT_CACHE_CAPACITY,
        ))
    }

    pub fn new_with_client(
        client: reqwest::Client,
        base_url: &str,
        positive_ttl: Duration,
        negative_ttl: Duration,
        capacity: usize,
    ) -> Self {
        Self {
            client,
            endpoint: format!("{}/api/auth/me/permissions", base_url.trim_end_matches('/')),
            positive_ttl,
            negative_ttl,
            cache: Arc::new(Mutex::new(PermissionCache::new(capacity))),
        }
    }

    /// SHA-256 cache key. The clear-text token is never used as a key or value.
    pub fn cache_key(token: &str) -> String {
        format!("{:x}", Sha256::digest(token.as_bytes()))
    }

    #[cfg(test)]
    fn cache_len(&self) -> usize {
        self.cache
            .lock()
            .expect("cache lock poisoned")
            .entries
            .len()
    }
}

#[async_trait]
impl PermissionsProvider for FalconPermissionsClient {
    async fn permissions(&self, token: &str) -> PermissionsResult {
        let key = Self::cache_key(token);
        if let Some(value) = self
            .cache
            .lock()
            .expect("cache lock poisoned")
            .get(&key, Instant::now())
        {
            return match value {
                CacheValue::Positive(permissions) => Ok(permissions),
                CacheValue::Negative(failure) => Err(failure),
            };
        }

        let result = match self
            .client
            .get(&self.endpoint)
            .bearer_auth(token)
            .send()
            .await
        {
            Err(_) => Err(PermissionsFailure::Unavailable),
            Ok(response) if response.status() == StatusCode::UNAUTHORIZED => {
                // A 401 carries the discriminating `error_code`; an unreadable body simply
                // yields no code, which the whitelist already treats as terminal.
                let code = response
                    .json::<serde_json::Value>()
                    .await
                    .ok()
                    .and_then(|body| error_code_of(&body));
                Err(classify_unauthorized(code.as_deref()))
            }
            Ok(response) if response.status() != StatusCode::OK => {
                Err(classify_status(response.status()))
            }
            Ok(response) => match response.json::<FalconPermissionsResponse>().await {
                Ok(body) => Ok(body.into_permissions()),
                // A malformed 200 is an unavailable permissions response. Keep it in the same
                // result path as transport/status failures so the negative TTL protects Falcon
                // from repeated calls for a broken upstream payload.
                Err(_) => Err(PermissionsFailure::Unavailable),
            },
        };

        let (value, ttl) = match &result {
            Ok(permissions) => (CacheValue::Positive(permissions.clone()), self.positive_ttl),
            Err(failure) => (CacheValue::Negative(*failure), self.negative_ttl),
        };
        self.cache
            .lock()
            .expect("cache lock poisoned")
            .insert(key, value, Instant::now() + ttl);
        result
    }
}

#[cfg(test)]
mod tests {

    /// Live evidence (2026-08-31, `https://dev.hdre-eomc.com/api/auth/me/permissions`,
    /// credential-free probes): the endpoint returns `401` with a body carrying an
    /// `error_code`, e.g. `{"detail": "未登入", "error_code": "auth.missing_token"}` and
    /// `{"detail": "Token 無效或已過期", "error_code": "auth.token_invalid"}`.
    ///
    /// Only `auth.token_invalid` is fixable by refreshing the token. Collapsing every 401
    /// into one bucket would leave the consumer unable to tell "refresh will help" from
    /// "refresh can never help", which is precisely how an infinite refresh loop is built
    /// against a deactivated account.
    #[test]
    fn only_token_invalid_is_classified_as_refreshable() {
        assert_eq!(
            classify_unauthorized(Some("auth.token_invalid")),
            PermissionsFailure::UnauthorizedRefreshable
        );

        for code in [
            "auth.missing_token",
            "auth.token_revoked",
            "auth.user_not_found",
            "auth.user_inactive",
            "auth.invalid_platform",
        ] {
            assert_eq!(
                classify_unauthorized(Some(code)),
                PermissionsFailure::UnauthorizedTerminal,
                "`{code}` must not invite a refresh"
            );
        }
    }

    /// The `auth.*` codes are **not** documented in the integration guide (verified across
    /// every branch that carries it). They can therefore change without a contract update,
    /// so the mapping is a whitelist: anything unrecognised — a new code, a renamed one, or
    /// a body with no `error_code` at all — is terminal.
    ///
    /// The failure modes are asymmetric. Treating a refreshable failure as terminal costs
    /// one re-login; treating a terminal failure as refreshable costs an infinite loop.
    #[test]
    fn unknown_and_absent_error_codes_default_to_terminal() {
        assert_eq!(
            classify_unauthorized(Some("auth.some_future_code")),
            PermissionsFailure::UnauthorizedTerminal
        );
        assert_eq!(
            classify_unauthorized(None),
            PermissionsFailure::UnauthorizedTerminal
        );
    }

    /// A non-401 still classifies by status alone; only the 401 branch consults the body.
    #[test]
    fn non_401_statuses_remain_unavailable() {
        assert_eq!(
            classify_status(StatusCode::INTERNAL_SERVER_ERROR),
            PermissionsFailure::Unavailable
        );
        assert_eq!(
            classify_status(StatusCode::BAD_GATEWAY),
            PermissionsFailure::Unavailable
        );
    }

    /// The error body shape is the one the live endpoint returns.
    #[test]
    fn error_code_is_read_from_the_documented_body_shape() {
        let body = serde_json::json!({"detail": "未登入", "error_code": "auth.missing_token"});
        assert_eq!(error_code_of(&body).as_deref(), Some("auth.missing_token"));

        let no_code = serde_json::json!({"detail": "something else"});
        assert_eq!(error_code_of(&no_code), None);
    }
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;

    fn spawn_responder(
        status: &'static str,
        body: &'static str,
        requests: usize,
    ) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind local listener");
        let address = format!(
            "http://{}",
            listener.local_addr().expect("listener address")
        );
        let handle = thread::spawn(move || {
            for _ in 0..requests {
                let (mut stream, _) = listener.accept().expect("accept request");
                let mut request = [0_u8; 8192];
                let _ = stream.read(&mut request).expect("read request");
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream
                    .write_all(response.as_bytes())
                    .expect("write response");
            }
        });
        (address, handle)
    }

    fn spawn_capturing_responder(
        status: &'static str,
        body: &'static str,
    ) -> (String, Arc<Mutex<Option<String>>>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind local listener");
        let address = format!(
            "http://{}",
            listener.local_addr().expect("listener address")
        );
        let captured = Arc::new(Mutex::new(None));
        let thread_captured = captured.clone();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut request = [0_u8; 8192];
            let bytes_read = stream.read(&mut request).expect("read request");
            *thread_captured
                .lock()
                .expect("capture lock should not be poisoned") =
                Some(String::from_utf8_lossy(&request[..bytes_read]).into_owned());
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .expect("write response");
        });
        (address, captured, handle)
    }

    fn spawn_slow_responder(delay: Duration) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind local listener");
        let address = format!(
            "http://{}",
            listener.local_addr().expect("listener address")
        );
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut request = [0_u8; 8192];
            let _ = stream.read(&mut request).expect("read request");
            thread::sleep(delay);
        });
        (address, handle)
    }

    #[test]
    /// S-RUNTIME-SEC-02 AC-001
    fn documented_response_parses_role_objects_and_filters_to_readable_permissions() {
        let response = serde_json::json!({
            "user_id": 123,
            "roles": [{
                "id": 10,
                "code": "viewer",
                "name": "檢視者",
                "default_pages": ["/dashboard/"]
            }],
            "permissions": [{
                "code": "admin:permissions",
                "name": "權限管理",
                "category": "admin",
                "page_path": "/admin/permissions",
                "can_read": true,
                "can_write": false
            }, {
                "code": "admin:write",
                "name": "寫入",
                "category": "admin",
                "page_path": null,
                "can_read": false,
                "can_write": true
            }]
        });

        let permissions = parse_permissions_response(response).expect("documented 200 must parse");
        assert_eq!(permissions.user_id, 123);
        assert_eq!(permissions.codes.len(), 1);
        assert!(permissions.codes.contains("admin:permissions"));
    }

    #[test]
    fn classify_status_defaults_a_401_to_terminal() {
        // Reached only for a 401 whose body could not be read; the client's own 401 arm
        // consults `error_code` first.
        assert_eq!(
            classify_status(StatusCode::UNAUTHORIZED),
            PermissionsFailure::UnauthorizedTerminal
        );
    }

    /// The refreshable path end to end over HTTP, not just through the pure classifier.
    ///
    /// This is the one 401 a consumer is allowed to spend its single refresh on. Without a
    /// wire-level test, a regression in reading the 401 body would silently collapse every
    /// refreshable 401 into terminal and the whole suite would stay green — the consumer
    /// would simply stop refreshing, which is invisible until someone's session dies.
    #[tokio::test]
    async fn wire_401_with_token_invalid_is_refreshable() {
        let (base_url, server) = spawn_responder(
            "401 Unauthorized",
            r#"{"detail":"Token 無效或已過期","error_code":"auth.token_invalid"}"#,
            1,
        );
        let client = FalconPermissionsClient::new_with_client(
            reqwest::Client::new(),
            &base_url,
            Duration::from_secs(60),
            Duration::from_secs(10),
            2,
        );

        assert_eq!(
            client.permissions("expired-token").await,
            Err(PermissionsFailure::UnauthorizedRefreshable)
        );
        server.join().expect("responder thread");
    }

    /// The mirror case: a real code that is not the whitelisted one stays terminal over the
    /// wire. Pins that the client reads the body and applies the whitelist, rather than
    /// happening to return terminal because it ignored the body entirely.
    #[tokio::test]
    /// S-RUNTIME-SEC-02 AC-025
    async fn wire_401_with_another_code_is_terminal() {
        let (base_url, server) = spawn_responder(
            "401 Unauthorized",
            r#"{"detail":"帳號已停用","error_code":"auth.user_inactive"}"#,
            1,
        );
        let client = FalconPermissionsClient::new_with_client(
            reqwest::Client::new(),
            &base_url,
            Duration::from_secs(60),
            Duration::from_secs(10),
            2,
        );

        assert_eq!(
            client.permissions("deactivated-user-token").await,
            Err(PermissionsFailure::UnauthorizedTerminal)
        );
        server.join().expect("responder thread");
    }

    /// An additive upstream field must not break the success path. Rejecting unknown fields
    /// would turn one routine backward-compatible change into a 503 for every user.
    #[test]
    fn unknown_fields_in_a_200_body_still_parse() {
        let response = serde_json::json!({
            "user_id": 123,
            "tenant": "hdre",
            "roles": [{"id": 10, "code": "viewer", "name": "檢視者", "extra": true}],
            "permissions": [{
                "code": "admin:permissions",
                "name": "權限管理",
                "category": "admin",
                "page_path": "/admin/permissions",
                "can_read": true,
                "can_write": false,
                "granted_at": "2026-08-31T00:00:00Z"
            }]
        });

        let permissions =
            parse_permissions_response(response).expect("additive fields must not break parsing");
        assert!(permissions.codes.contains("admin:permissions"));
    }

    #[tokio::test]
    /// S-RUNTIME-SEC-02 AC-001
    async fn http_client_uses_documented_permissions_path_and_bearer_transport() {
        let body = r#"{
            "user_id": 123,
            "roles": [],
            "permissions": []
        }"#;
        let (base_url, captured, server) = spawn_capturing_responder("200 OK", body);
        let client = FalconPermissionsClient::new_with_client(
            reqwest::Client::new(),
            &base_url,
            Duration::from_secs(60),
            Duration::from_secs(10),
            2,
        );

        client
            .permissions("documented-token")
            .await
            .expect("documented response should be accepted");
        server
            .join()
            .expect("capturing upstream request should finish");

        let request = captured
            .lock()
            .expect("capture lock should not be poisoned")
            .clone()
            .expect("upstream request should be captured")
            .to_ascii_lowercase();
        assert!(request.starts_with("get /api/auth/me/permissions http/1.1"));
        assert!(request
            .lines()
            .any(|line| line == "authorization: bearer documented-token"));
    }

    #[tokio::test]
    /// S-RUNTIME-SEC-02 AC-011
    async fn http_client_maps_request_timeout_to_unavailable() {
        let (base_url, server) = spawn_slow_responder(Duration::from_millis(100));
        let http = reqwest::Client::builder()
            .timeout(Duration::from_millis(20))
            .build()
            .expect("short-timeout client should build");
        let client = FalconPermissionsClient::new_with_client(
            http,
            &base_url,
            Duration::from_secs(60),
            Duration::from_secs(10),
            2,
        );

        assert_eq!(
            client.permissions("timeout-token").await,
            Err(PermissionsFailure::Unavailable)
        );
        server.join().expect("slow responder should finish");
    }

    #[tokio::test]
    /// S-RUNTIME-SEC-02 AC-003, AC-002（cache 半邊）
    async fn positive_cache_reuses_a_lookup_and_never_stores_the_clear_token() {
        let body = r#"{
            "user_id": 123,
            "roles": [],
            "permissions": [{
                "code": "finance:read",
                "name": "Finance",
                "category": "finance",
                "page_path": null,
                "can_read": true,
                "can_write": false
            }]
        }"#;
        let (base_url, server) = spawn_responder("200 OK", body, 1);
        let client = FalconPermissionsClient::new_with_client(
            reqwest::Client::new(),
            &base_url,
            Duration::from_secs(60),
            Duration::from_secs(10),
            2,
        );

        let first = client.permissions("secret-user-token").await;
        let second = client.permissions("secret-user-token").await;

        assert_eq!(first, second);
        assert_eq!(client.cache_len(), 1);
        let key = FalconPermissionsClient::cache_key("secret-user-token");
        assert!(!key.contains("secret-user-token"));
        server
            .join()
            .expect("single upstream request should finish");
    }

    #[tokio::test]
    /// S-RUNTIME-SEC-02 AC-020
    async fn unauthorized_cache_replays_the_same_failure_from_one_upstream_call() {
        let (base_url, server) =
            spawn_responder("401 Unauthorized", r#"{"detail":"do not parse"}"#, 1);
        let client = FalconPermissionsClient::new_with_client(
            reqwest::Client::new(),
            &base_url,
            Duration::from_secs(60),
            Duration::from_secs(10),
            2,
        );

        assert_eq!(
            client.permissions("expired-token").await,
            Err(PermissionsFailure::UnauthorizedTerminal)
        );
        assert_eq!(
            client.permissions("expired-token").await,
            Err(PermissionsFailure::UnauthorizedTerminal)
        );
        assert_eq!(client.cache_len(), 1);
        server
            .join()
            .expect("single upstream request should finish");
    }

    #[tokio::test]
    async fn malformed_success_is_negative_cached_as_unavailable() {
        let (base_url, server) = spawn_responder("200 OK", r#"{"not":"the documented shape"}"#, 1);
        let client = FalconPermissionsClient::new_with_client(
            reqwest::Client::new(),
            &base_url,
            Duration::from_secs(60),
            Duration::from_secs(10),
            2,
        );

        assert_eq!(
            client.permissions("malformed-token").await,
            Err(PermissionsFailure::Unavailable)
        );
        assert_eq!(
            client.permissions("malformed-token").await,
            Err(PermissionsFailure::Unavailable)
        );
        assert_eq!(client.cache_len(), 1);
        server
            .join()
            .expect("single upstream request should finish");
    }

    #[tokio::test]
    async fn transport_failure_is_negative_cached_as_unavailable() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind local listener");
        let base_url = format!(
            "http://{}",
            listener.local_addr().expect("listener address")
        );
        drop(listener);
        let client = FalconPermissionsClient::new_with_client(
            reqwest::Client::new(),
            &base_url,
            Duration::from_secs(60),
            Duration::from_secs(10),
            2,
        );

        assert_eq!(
            client.permissions("unavailable-token").await,
            Err(PermissionsFailure::Unavailable)
        );
        assert_eq!(
            client.permissions("unavailable-token").await,
            Err(PermissionsFailure::Unavailable)
        );
        assert_eq!(client.cache_len(), 1);
    }

    #[tokio::test]
    async fn cache_evicts_the_least_recently_used_entry_at_its_bound() {
        let body = r#"{
            "user_id": 123,
            "roles": [],
            "permissions": [{
                "code": "finance:read",
                "name": "Finance",
                "category": "finance",
                "page_path": null,
                "can_read": true,
                "can_write": false
            }]
        }"#;
        let (base_url, server) = spawn_responder("200 OK", body, 4);
        let client = FalconPermissionsClient::new_with_client(
            reqwest::Client::new(),
            &base_url,
            Duration::from_secs(60),
            Duration::from_secs(10),
            2,
        );

        client.permissions("token-a").await.expect("first lookup");
        client.permissions("token-b").await.expect("second lookup");
        client
            .permissions("token-a")
            .await
            .expect("touch first lookup");
        client
            .permissions("token-c")
            .await
            .expect("evict least recently used");
        assert_eq!(client.cache_len(), 2);
        client
            .permissions("token-b")
            .await
            .expect("token b was evicted");
        server
            .join()
            .expect("all expected upstream requests should finish");
    }
}
