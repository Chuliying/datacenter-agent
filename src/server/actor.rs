//! Opaque actor identity derived from Falcon's user id.

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

/// Minimum pepper length required for actor-key derivation.
pub const MIN_PEPPER_BYTES: usize = 32;

/// Environment variable containing the actor-key HMAC pepper.
pub const ACTOR_KEY_PEPPER_ENV: &str = "ACTOR_KEY_PEPPER";

/// Stable, opaque actor identifier used for request-scoped isolation.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ActorKey(String);

impl ActorKey {
    /// Derive an actor key from a Falcon user id and the configured pepper.
    pub fn derive(user_id: i64, pepper: &[u8]) -> Result<Self> {
        let pepper = validate_pepper(pepper)?;
        let message = format!("falcon-user:{user_id}");
        let digest = hmac_sha256(&pepper, message.as_bytes());
        let encoded = base64url_without_padding(&digest);
        Ok(Self(format!("v1:{}", &encoded[..32])))
    }

    /// Borrow the serialized actor key.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Load and validate the actor-key pepper from the process environment.
pub fn load_actor_key_pepper() -> Result<Vec<u8>> {
    let value = std::env::var(ACTOR_KEY_PEPPER_ENV).map_err(|error| {
        anyhow::anyhow!(
            "env_error: {ACTOR_KEY_PEPPER_ENV} missing or unreadable; pepper must be at least {MIN_PEPPER_BYTES} bytes ({error})"
        )
    })?;
    validate_pepper(value.as_bytes())
}

/// Validate a pepper value without including its contents in any error.
pub fn validate_pepper(pepper: &[u8]) -> Result<Vec<u8>> {
    if pepper.len() < MIN_PEPPER_BYTES {
        anyhow::bail!(
            "env_error: {ACTOR_KEY_PEPPER_ENV} must be at least {MIN_PEPPER_BYTES} bytes"
        );
    }
    Ok(pepper.to_vec())
}

/// Validate an optional environment value; this pure seam keeps startup tests independent of
/// process-global environment mutation.
pub fn validate_actor_key_pepper_value(value: Option<&[u8]>) -> Result<Vec<u8>> {
    let value = value.with_context(|| {
        format!(
            "env_error: {ACTOR_KEY_PEPPER_ENV} missing; pepper must be at least {MIN_PEPPER_BYTES} bytes"
        )
    })?;
    validate_pepper(value)
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK_SIZE: usize = 64;
    let mut key_block = [0_u8; BLOCK_SIZE];
    if key.len() > BLOCK_SIZE {
        let digest: [u8; 32] = Sha256::digest(key).into();
        key_block[..digest.len()].copy_from_slice(&digest);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }

    let mut inner_pad = [0x36_u8; BLOCK_SIZE];
    let mut outer_pad = [0x5c_u8; BLOCK_SIZE];
    for (index, byte) in key_block.iter().enumerate() {
        inner_pad[index] ^= byte;
        outer_pad[index] ^= byte;
    }

    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(message);
    let inner_digest = inner.finalize();

    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner_digest);
    outer.finalize().into()
}

fn base64url_without_padding(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut encoded = String::with_capacity((bytes.len() * 8).div_ceil(6));
    for chunk in bytes.chunks(3) {
        let first = u32::from(chunk[0]);
        encoded.push(ALPHABET[((first >> 2) & 0x3f) as usize] as char);
        if chunk.len() == 1 {
            encoded.push(ALPHABET[((first & 0x03) << 4) as usize] as char);
            continue;
        }

        let second = u32::from(chunk[1]);
        encoded.push(ALPHABET[(((first & 0x03) << 4) | (second >> 4)) as usize] as char);
        if chunk.len() == 2 {
            encoded.push(ALPHABET[((second & 0x0f) << 2) as usize] as char);
            continue;
        }

        let third = u32::from(chunk[2]);
        encoded.push(ALPHABET[(((second & 0x0f) << 2) | (third >> 6)) as usize] as char);
        encoded.push(ALPHABET[(third & 0x3f) as usize] as char);
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    /// S-RUNTIME-SEC-02 AC-004
    fn derives_the_pinned_actor_key_for_a_known_input() {
        let pepper = b"0123456789abcdef0123456789abcdef";
        let key = ActorKey::derive(123, pepper).expect("known input should derive");
        assert_eq!(
            key.as_str(),
            "v1:tbuxT4ncZRJaObivzRDrhCemI-PK3GjT",
            "actor key format must be stable and opaque"
        );
    }

    #[test]
    fn rejects_peppers_shorter_than_the_minimum_without_echoing_them() {
        let short = b"short-pepper";
        let error = validate_pepper(short).expect_err("short pepper must fail");
        let message = error.to_string();
        assert!(message.contains(ACTOR_KEY_PEPPER_ENV));
        assert!(message.contains("32"));
        assert!(!message.contains("short-pepper"));
    }

    #[test]
    fn rejects_an_empty_pepper_without_echoing_it() {
        let error = validate_pepper(b"").expect_err("empty pepper must fail");
        let message = error.to_string();
        assert!(message.contains(ACTOR_KEY_PEPPER_ENV));
        assert!(message.contains("32"));
    }

    #[test]
    /// S-RUNTIME-SEC-02 AC-014
    fn rejects_a_missing_pepper_without_echoing_a_value() {
        let error = validate_actor_key_pepper_value(None)
            .expect_err("missing pepper must fail startup validation");
        let message = error.to_string();
        assert!(message.contains(ACTOR_KEY_PEPPER_ENV));
        assert!(message.contains("32"));
    }
}
