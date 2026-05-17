// SPDX-License-Identifier: AGPL-3.0-or-later

//! Webhook signature verification for GitHub App webhooks.
//!
//! GitHub signs every webhook payload with HMAC-SHA256 using the app's webhook
//! secret. This module provides constant-time signature verification to prevent
//! timing attacks.

use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

/// Verify a GitHub webhook signature against the raw request body.
///
/// GitHub sends the signature in the `X-Hub-Signature-256` header as
/// `sha256=<hex-encoded HMAC>`. This function:
///
/// 1. Parses the `sha256=` prefix from the header value
/// 2. Computes HMAC-SHA256 of the body using the provided secret
/// 3. Compares the computed and received MACs in constant time
///
/// Returns `false` if:
/// - The header is missing the `sha256=` prefix
/// - The hex decoding fails
/// - The signature does not match
pub fn verify_signature(secret: &[u8], body: &[u8], signature_header: &str) -> bool {
    let hex_sig = match signature_header.strip_prefix("sha256=") {
        Some(h) => h,
        None => return false,
    };

    let received_mac = match hex::decode(hex_sig) {
        Ok(bytes) => bytes,
        Err(_) => return false,
    };

    let mut mac = match HmacSha256::new_from_slice(secret) {
        Ok(m) => m,
        Err(_) => return false,
    };
    mac.update(body);
    let computed_mac = mac.finalize().into_bytes();

    // Constant-time comparison to prevent timing attacks.
    computed_mac.as_slice().ct_eq(&received_mac).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: compute the real HMAC-SHA256 hex for a given secret and body.
    fn compute_signature(secret: &[u8], body: &[u8]) -> String {
        let mut mac =
            HmacSha256::new_from_slice(secret).expect("HMAC can take key of any size");
        mac.update(body);
        let result = mac.finalize().into_bytes();
        format!("sha256={}", hex::encode(result))
    }

    #[test]
    fn test_verify_valid_signature() {
        let secret = b"test-webhook-secret-not-real";
        let body = b"hello world";
        let sig = compute_signature(secret, body);

        assert!(verify_signature(secret, body, &sig));
    }

    #[test]
    fn test_verify_invalid_signature() {
        let secret = b"test-webhook-secret-not-real";
        let body = b"hello world";
        // Use a different body to produce a wrong signature
        let wrong_sig = compute_signature(secret, b"different body");

        assert!(!verify_signature(secret, body, &wrong_sig));
    }

    #[test]
    fn test_verify_malformed_header() {
        let secret = b"test-webhook-secret-not-real";
        let body = b"hello world";

        // Missing "sha256=" prefix
        assert!(!verify_signature(secret, body, "md5=abc123"));
        assert!(!verify_signature(secret, body, "abc123"));
        assert!(!verify_signature(secret, body, ""));
    }

    #[test]
    fn test_verify_empty_body() {
        let secret = b"test-webhook-secret-not-real";
        let body = b"";
        let sig = compute_signature(secret, body);

        assert!(verify_signature(secret, body, &sig));
    }

    #[test]
    fn test_verify_invalid_hex_in_header() {
        let secret = b"test-webhook-secret-not-real";
        let body = b"hello world";

        assert!(!verify_signature(secret, body, "sha256=not-valid-hex!!!"));
    }

    #[test]
    fn test_verify_wrong_secret() {
        let secret = b"test-webhook-secret-not-real";
        let body = b"hello world";
        let sig = compute_signature(b"wrong-secret-value", body);

        assert!(!verify_signature(secret, body, &sig));
    }
}
