//! Shared parsing for `openssl x509 -fingerprint` output.

use std::sync::LazyLock;

static FINGERPRINT_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"Fingerprint=([0-9A-Fa-f:]+)").unwrap());

/// Extract a fingerprint from `openssl x509 -fingerprint` output (e.g.
/// `SHA1 Fingerprint=AA:BB:CC:...`), as uppercase hex with the colons
/// removed. `None` if the output doesn't contain a fingerprint.
pub fn parse_fingerprint(output: &str) -> Option<String> {
    let hex = FINGERPRINT_RE
        .captures(output)?
        .get(1)?
        .as_str()
        .replace(':', "")
        .to_ascii_uppercase();
    (!hex.is_empty()).then_some(hex)
}
