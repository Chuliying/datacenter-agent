//! Route-table contract for the retired endpoints (AC-001, partial evidence).
//!
//! `POST /insight`, `/insight/stream`, `/report` and `/report/stream` were retired. AC-001 asks
//! for an HTTP-level check that those paths answer `404` regardless of the `Authorization` header.
//! That needs a served router, and serving one needs a full [`AppState`] — whose `mcp` field is an
//! `McpHandle` wrapping a private `Peer<RoleClient>` that only `McpClient::connect_http` produces.
//! So an HTTP-level assertion requires either a live MCP server or an in-memory rmcp server behind
//! a dev-dependency on rmcp's `server` feature (this crate depends on rmcp as a client only).
//!
//! Until one of those exists, this guard covers the regression that actually matters: it pins the
//! registered route table so a retired path cannot be reintroduced unnoticed. It asserts on the
//! source of `build_router`, following the same convention as `deployment_contract.rs`, which
//! asserts on the `Dockerfile`.
//!
//! What it does NOT prove: the status code, or that the `404` fallback stays ahead of the auth
//! layer. Those remain open in `docs/work/retire-superseded-agent-endpoints/prd.md` `## Delivery`.

/// Registered paths, extracted from the body of `pub fn build_router` only.
///
/// The `#[cfg(test)]` module further down `route.rs` registers its own throwaway routes; scoping to
/// the function body keeps those out of the assertion.
fn registered_paths() -> Vec<String> {
    let source = std::fs::read_to_string("src/server/route.rs")
        .expect("src/server/route.rs should be readable");

    let start = source
        .find("pub fn build_router")
        .expect("route.rs should define build_router");
    let body = &source[start..];
    let end = body
        .find("\n#[cfg(test)]")
        .unwrap_or_else(|| panic!("route.rs should keep its test module after build_router"));
    let body = &body[..end];

    body.match_indices(".route(\"")
        .map(|(index, marker)| {
            let rest = &body[index + marker.len()..];
            let close = rest
                .find('"')
                .expect("a .route( literal should be terminated");
            rest[..close].to_string()
        })
        .collect()
}

#[test]
fn retired_endpoints_are_not_registered() {
    let paths = registered_paths();

    for retired in ["/insight", "/insight/stream", "/report", "/report/stream"] {
        assert!(
            !paths.iter().any(|path| path == retired),
            "{retired} was retired but is registered again in build_router; \
             callers use /agent/stream (streaming) or /v1/chat/completions (non-streaming)"
        );
    }
}

#[test]
fn build_router_registers_exactly_the_surviving_paths() {
    let mut paths = registered_paths();
    paths.sort();

    // Six routes: five standard (three probes plus the two streaming front doors) and the
    // OpenAI-compatible endpoint. Update this list together with docs/reference/endpoints/index.md
    // when a route is added or removed on purpose.
    let mut expected = vec![
        "/agent/stream",
        "/greeting",
        "/health",
        "/ready",
        "/ss-chat/stream",
        "/v1/chat/completions",
    ];
    expected.sort();

    assert_eq!(
        paths, expected,
        "the registered route table drifted from the documented one \
         (docs/reference/endpoints/index.md)"
    );
}
