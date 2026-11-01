//! Tokens and signatures.
//!
//! * Confirmation tokens are random; only their SHA-256 is stored.
//! * Unsubscribe tokens are `"<subscriber id>.<hmac>"`, so they need no storage
//!   and can be verified statelessly, and they never expire (a recipient must
//!   always be able to opt out).
//! * IP addresses are only ever stored as keyed hashes (for rate limiting).

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use crate::Platform;

type HmacSha256 = Hmac<Sha256>;

/// A new random token, 256 bits, hex encoded.
pub fn generate_token(platform: &impl Platform) -> String {
    hex::encode(platform.random_bytes(32))
}

/// A new random identifier, 128 bits, hex encoded.
pub fn generate_id(platform: &impl Platform) -> String {
    hex::encode(platform.random_bytes(16))
}

/// SHA-256 of a token, hex encoded. This is what gets stored.
pub fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

fn mac(secret: &str, message: &str) -> HmacSha256 {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key size");
    mac.update(message.as_bytes());
    mac
}

/// Signed, stateless unsubscribe token for a subscriber id.
pub fn sign_unsubscribe(secret: &str, subscriber_id: &str) -> String {
    let sig = hex::encode(
        mac(secret, &format!("unsubscribe:{subscriber_id}"))
            .finalize()
            .into_bytes(),
    );
    format!("{subscriber_id}.{sig}")
}

/// Verifies a token from [`sign_unsubscribe`] and returns the subscriber id.
pub fn verify_unsubscribe(secret: &str, token: &str) -> Option<String> {
    let (id, sig_hex) = token.split_once('.')?;
    if id.is_empty() || id.len() > 64 {
        return None;
    }
    let sig = hex::decode(sig_hex).ok()?;
    // `verify_slice` compares in constant time.
    mac(secret, &format!("unsubscribe:{id}"))
        .verify_slice(&sig)
        .ok()?;
    Some(id.to_string())
}

/// Keyed hash of an IP address, for rate-limit buckets.
pub fn hash_ip(secret: &str, ip: &str) -> String {
    hex::encode(mac(secret, &format!("ip:{ip}")).finalize().into_bytes())
}

/// Constant-time string comparison (for admin bearer tokens).
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    // Hash first so the comparison time does not depend on length either.
    let (ha, hb) = (Sha256::digest(a), Sha256::digest(b));
    let mut diff = (a.len() ^ b.len()) as u8;
    for (x, y) in ha.iter().zip(hb.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsubscribe_tokens_round_trip() {
        let t = sign_unsubscribe("secret", "abc123");
        assert_eq!(verify_unsubscribe("secret", &t).as_deref(), Some("abc123"));
    }

    #[test]
    fn unsubscribe_tokens_reject_tampering() {
        let t = sign_unsubscribe("secret", "abc123");
        assert!(verify_unsubscribe("other-secret", &t).is_none());
        // Swap the id, keep the signature.
        let forged = t.replacen("abc123", "abc124", 1);
        assert!(verify_unsubscribe("secret", &forged).is_none());
        // Truncated / garbage.
        assert!(verify_unsubscribe("secret", "abc123").is_none());
        assert!(verify_unsubscribe("secret", "abc123.").is_none());
        assert!(verify_unsubscribe("secret", "abc123.zz").is_none());
        assert!(verify_unsubscribe("secret", ".deadbeef").is_none());
        assert!(verify_unsubscribe("secret", "").is_none());
    }

    #[test]
    fn hashes_are_stable_and_distinct() {
        assert_eq!(hash_token("a"), hash_token("a"));
        assert_ne!(hash_token("a"), hash_token("b"));
        assert_eq!(hash_token("a").len(), 64);
        assert_ne!(hash_ip("s", "1.2.3.4"), hash_ip("s", "1.2.3.5"));
        assert_ne!(hash_ip("s", "1.2.3.4"), hash_ip("t", "1.2.3.4"));
    }

    #[test]
    fn constant_time_eq_behaves_like_eq() {
        assert!(constant_time_eq("token", "token"));
        assert!(!constant_time_eq("token", "tokex"));
        assert!(!constant_time_eq("token", "token2"));
        assert!(!constant_time_eq("", "x"));
        assert!(constant_time_eq("", ""));
    }
}
