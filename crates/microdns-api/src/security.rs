use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use tracing::error;

use crate::AppState;

/// Convert an internal error into a generic 500 response, logging the real error.
pub fn internal_error(e: impl std::fmt::Display) -> (StatusCode, String) {
    error!("internal error: {e}");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal server error".to_string(),
    )
}

/// Middleware: enforce API key authentication when configured.
/// Skips auth for /health and /dashboard endpoints.
pub async fn api_key_auth(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let expected_key = match &state.api_key {
        Some(key) => key,
        None => return Ok(next.run(request).await),
    };

    let path = request.uri().path();

    // Allow unauthenticated access to health check and dashboard
    if path == "/api/v1/health" || path == "/dashboard" {
        return Ok(next.run(request).await);
    }

    let provided = request
        .headers()
        .get("x-api-key")
        .and_then(|v| v.to_str().ok());

    match provided {
        Some(key) if key == expected_key.as_str() => Ok(next.run(request).await),
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}

/// Validate a DNS name (zone or record name).
/// Returns Ok(()) if valid, Err(message) if invalid.
pub fn validate_dns_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("name cannot be empty".into());
    }

    // Allow "@" as zone apex shorthand
    if name == "@" {
        return Ok(());
    }

    // Allow wildcard prefix
    let check_name = name.strip_prefix("*.").unwrap_or(name);

    let clean = check_name.trim_end_matches('.');
    if clean.is_empty() {
        return Err("name cannot be empty".into());
    }
    if clean.len() > 253 {
        return Err("name exceeds 253 characters".into());
    }

    for label in clean.split('.') {
        if label.is_empty() {
            return Err("empty label in name".into());
        }
        if label.len() > 63 {
            return Err("label exceeds 63 characters".into());
        }
        if !label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(format!("invalid characters in label: {label}"));
        }
    }

    Ok(())
}

/// Pagination query parameters for list endpoints.
#[derive(Debug, serde::Deserialize)]
pub struct Pagination {
    #[serde(default)]
    pub offset: usize,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    100
}

impl Pagination {
    /// Apply pagination to a Vec, clamping limit to MAX_PAGE_SIZE.
    pub fn apply<T>(&self, items: Vec<T>) -> Vec<T> {
        let limit = self.limit.min(1000);
        items.into_iter().skip(self.offset).take(limit).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_dns_name_valid() {
        assert!(validate_dns_name("example.com").is_ok());
        assert!(validate_dns_name("sub.example.com").is_ok());
        assert!(validate_dns_name("@").is_ok());
        assert!(validate_dns_name("*.example.com").is_ok());
        assert!(validate_dns_name("a-b.example.com").is_ok());
        assert!(validate_dns_name("www").is_ok());
    }

    #[test]
    fn test_validate_dns_name_invalid() {
        assert!(validate_dns_name("").is_err());
        assert!(validate_dns_name(&"a".repeat(254)).is_err());
        assert!(validate_dns_name("bad name.com").is_err());
        assert!(validate_dns_name("bad;name.com").is_err());
    }
}
