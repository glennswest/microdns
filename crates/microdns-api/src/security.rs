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

    // Allow all GET requests without auth (read-only); mutations still require key
    if request.method() == axum::http::Method::GET {
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

    // Allow bare wildcard "*" (wildcard at zone level)
    if name == "*" {
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

    for label in split_labels(clean)? {
        if label.is_empty() {
            return Err("empty label in name".into());
        }
        // The limit is on the wire form. `\032` is four characters here and one
        // byte there, which matters for DNS-SD instance names — they routinely
        // carry spaces and UTF-8.
        if wire_len(&label)? > 63 {
            return Err("label exceeds 63 characters".into());
        }
    }

    Ok(())
}

/// Split a name into labels on unescaped dots.
///
/// A dot inside a label is written `\.`, so splitting naively would tear such a
/// label in half and then reject both halves.
fn split_labels(name: &str) -> Result<Vec<String>, String> {
    let mut labels = Vec::new();
    let mut current = String::new();
    let mut chars = name.chars();

    while let Some(c) = chars.next() {
        match c {
            '.' => {
                labels.push(std::mem::take(&mut current));
            }
            '\\' => {
                current.push(c);
                match chars.next() {
                    Some(next) => current.push(next),
                    None => return Err("name ends with a dangling escape".into()),
                }
            }
            _ => current.push(c),
        }
    }
    labels.push(current);
    Ok(labels)
}

/// Length of a label once its escapes are decoded, validating them on the way.
///
/// Presentation format escapes anything outside the plain host character set:
/// `\NNN` for a byte written in octal (a space is `\040`, UTF-8 arrives as a run
/// of them — `\342\200\231` is a curly apostrophe), and `\X` for a literal
/// punctuation character. Both are ordinary DNS, and both are exactly what
/// hickory produces when it prints a name: a printer calling itself
/// "Epson ET-5170 Series" is not malformed.
fn wire_len(label: &str) -> Result<usize, String> {
    let chars: Vec<char> = label.chars().collect();
    let mut i = 0;
    let mut len = 0;

    while i < chars.len() {
        let c = chars[i];
        if c == '\\' {
            let rest = &chars[i + 1..];
            let octal: String = rest.iter().take(3).copied().collect();
            if octal.len() == 3 && octal.chars().all(|c| c.is_ascii_digit()) {
                let value = u16::from_str_radix(&octal, 8)
                    .map_err(|_| format!("invalid octal escape \\{octal} in label"))?;
                if value > 255 {
                    return Err(format!("escape \\{octal} is not a byte"));
                }
                i += 4;
            } else if let Some(next) = rest.first() {
                // `\ `, `\.`, `\\`, `\@` and friends: one escaped character.
                if next.is_ascii_digit() {
                    return Err("incomplete octal escape in label".into());
                }
                i += 2;
            } else {
                return Err("label ends with a dangling escape".into());
            }
            len += 1;
            continue;
        }

        if !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '*') {
            return Err(format!("invalid characters in label: {label}"));
        }
        i += 1;
        len += 1;
    }

    Ok(len)
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
        assert!(validate_dns_name("*").is_ok());
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

    #[test]
    fn escaped_dns_sd_instance_names_are_accepted() {
        // Escapes are octal, which is what hickory prints: \040 is a space.
        assert!(validate_dns_name("epson\\040et-5170\\040series._printer._tcp").is_ok());
        // UTF-8 arrives as a run of them (\342\200\231 is a curly apostrophe).
        assert!(validate_dns_name("glenn\\342\\200\\231s\\040mac._ssh._tcp").is_ok());
        // An escaped dot belongs to its label rather than splitting it.
        assert!(validate_dns_name("host\\.name.local").is_ok());
        // An escaped punctuation character, as in AirPlay instance names.
        assert!(validate_dns_name("02424c526151\\@gwest-mac._raop._tcp").is_ok());
        // Nonsense is still nonsense.
        assert!(validate_dns_name("bad name").is_err());
        assert!(validate_dns_name("trailing\\").is_err());
        assert!(validate_dns_name("host\\999").is_err(), "9 is not an octal digit");
        assert!(validate_dns_name("host\\77").is_err(), "escape must be three digits");
    }
}
