use anyhow::Result;
use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;

pub fn hash(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow::anyhow!("argon2 hash: {e}"))?
        .to_string();
    Ok(hash)
}

pub fn verify(password: &str, hash: &str) -> Result<bool> {
    let parsed = PasswordHash::new(hash).map_err(|e| anyhow::anyhow!("parse argon2 hash: {e}"))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

/// Spend roughly the same time a real `verify` would, then discard the result.
/// Called on login when the email is unknown, so response timing does not reveal
/// whether an account exists (defeats user enumeration via timing).
pub fn waste_time_verifying(password: &str) {
    use std::sync::OnceLock;
    static DUMMY_HASH: OnceLock<String> = OnceLock::new();
    let h = DUMMY_HASH.get_or_init(|| hash("timing-equalizer-not-a-real-password").unwrap_or_default());
    if !h.is_empty() {
        let _ = verify(password, h);
    }
}
