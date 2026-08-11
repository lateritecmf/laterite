//! Argon2id password hashing.
//!
//! `Argon2::default()` is Argon2id at the current OWASP-aligned default
//! parameters. Hashes are stored in PHC string form, which embeds the
//! algorithm, parameters, and salt, so verification is self-describing and
//! parameters can be raised over time without a migration.

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;

use crate::error::AuthError;

/// Hashes a plaintext password into a PHC string for storage.
pub fn hash_password(plain: &str) -> Result<String, AuthError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(plain.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|e| AuthError::PasswordHash(e.to_string()))
}

/// Verifies a plaintext password against a stored PHC hash.
///
/// A malformed stored hash is an internal error, not a failed match, so a
/// corrupted record is never silently treated as a wrong password.
pub fn verify_password(plain: &str, stored_hash: &str) -> Result<bool, AuthError> {
    let parsed =
        PasswordHash::new(stored_hash).map_err(|e| AuthError::PasswordHash(e.to_string()))?;
    Ok(Argon2::default()
        .verify_password(plain.as_bytes(), &parsed)
        .is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_verifiable_and_salted() {
        let a = hash_password("correct horse battery staple").unwrap();
        let b = hash_password("correct horse battery staple").unwrap();
        assert_ne!(a, b, "each hash must use a fresh salt");
        assert!(a.starts_with("$argon2id$"));
        assert!(verify_password("correct horse battery staple", &a).unwrap());
    }

    #[test]
    fn wrong_password_does_not_verify() {
        let hash = hash_password("s3cret").unwrap();
        assert!(!verify_password("guess", &hash).unwrap());
    }

    #[test]
    fn malformed_hash_is_an_error() {
        assert!(matches!(
            verify_password("x", "not-a-phc-string"),
            Err(AuthError::PasswordHash(_))
        ));
    }
}
