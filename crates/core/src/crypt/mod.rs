//! Encryption and signing under the application key.
//!
//! One master key (`app.key`, 32 bytes, base64) with an id, plus previous
//! keys kept for reading data written under them. Every operation names a
//! purpose; a subkey is derived per purpose, so a value encrypted for one
//! purpose never decrypts under another and a token signed for one flow never
//! verifies in another.
//!
//! ```ignore
//! let sealed = crypt.encrypt("api-secret", b"table.column.42", b"hunter2")?;
//! let plain = crypt.decrypt("api-secret", b"table.column.42", &sealed)?;
//! let token = crypt.token("password-reset", b"user:7", Duration::from_secs(3600));
//! let payload = crypt.check_token("password-reset", &token)?;
//! ```
//!
//! AEAD is XChaCha20-Poly1305 with a random 24-byte nonce; signing is keyed
//! BLAKE3. A sealed value is `[version][key id][nonce][ciphertext]` and a
//! signature `[version][key id][tag]`, both base64url without padding.

use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::query::{bind_values, build, AnyRowExt};
use crate::{CoreResult, Db, Module, ModuleId};
use sea_query::{Alias, Expr, Iden, OnConflict, Query};

pub mod migrations;

/// The text a sealed field shows in place of its value; submitting it back
/// means "unchanged".
pub const MASK: &str = "\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}";

/// The purpose a sealed form field uses; its AAD is the field's name.
pub const FIELD_PURPOSE: &str = "field";

/// The registry of sealed columns: what `lat crypt rotate` re-seals.
#[derive(Iden)]
pub(crate) enum CryptColumns {
    #[iden = "crypt_columns"]
    Table,
    TableName,
    ColumnName,
    Purpose,
    RegisteredAt,
}

/// The module that owns the registry table, listed among an application's
/// built-ins.
pub struct CryptModule;

impl Module for CryptModule {
    fn id(&self) -> ModuleId {
        ModuleId::new(migrations::MODULE_ID)
    }
    fn migrations(&self) -> crate::MigrationSet {
        migrations::migrations()
    }
}

/// Records that `table.column` holds values sealed for `purpose` (with the
/// column's name as AAD), so a rotation finds them. Idempotent.
pub async fn register_column(db: &Db, table: &str, column: &str, purpose: &str) -> CoreResult<()> {
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let (sql, values) = build(
        db.backend,
        Query::insert()
            .into_table(CryptColumns::Table)
            .columns([
                CryptColumns::TableName,
                CryptColumns::ColumnName,
                CryptColumns::Purpose,
                CryptColumns::RegisteredAt,
            ])
            .values_panic([table.into(), column.into(), purpose.into(), now.into()])
            .on_conflict(
                OnConflict::columns([CryptColumns::TableName, CryptColumns::ColumnName])
                    .update_columns([CryptColumns::Purpose, CryptColumns::RegisteredAt])
                    .to_owned(),
            )
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

/// Every registered sealed column: `(table, column, purpose)`.
pub async fn registered_columns(db: &Db) -> CoreResult<Vec<(String, String, String)>> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns([
                CryptColumns::TableName,
                CryptColumns::ColumnName,
                CryptColumns::Purpose,
            ])
            .from(CryptColumns::Table)
            .to_owned(),
    );
    let rows = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?;
    rows.iter()
        .map(|r| {
            Ok((
                r.get_text("table_name")?,
                r.get_text("column_name")?,
                r.get_text("purpose")?,
            ))
        })
        .collect()
}

/// What a rotation did: rows re-sealed under the current key, rows already
/// under it, and rows that would not open (left as they were).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Rotated {
    pub resealed: u64,
    pub current: u64,
    pub unreadable: u64,
}

/// Re-seals every registered column's values under the current key: a value
/// sealed under a previous key is opened and sealed again; one already under
/// the current key is left; one that does not open is counted and left.
pub async fn rotate(db: &Db, crypt: &Crypt) -> CoreResult<Rotated> {
    let mut out = Rotated::default();
    for (table, column, purpose) in registered_columns(db).await? {
        let (t, c) = (Alias::new(&table), Alias::new(&column));
        let id = Alias::new("id");
        let (sql, values) = build(
            db.backend,
            Query::select()
                .column(id.clone())
                .column(c.clone())
                .from(t.clone())
                .and_where(Expr::col(c.clone()).is_not_null())
                .to_owned(),
        );
        let rows = bind_values(sqlx::query(&sql), values)
            .fetch_all(&db.pool)
            .await?;
        for row in rows {
            let row_id: i64 = row.get_int("id")?;
            let sealed = row.get_text(&column)?;
            if sealed.is_empty() {
                continue;
            }
            if crypt.key_id_of(&sealed) == Some(crypt.current_id()) {
                out.current += 1;
                continue;
            }
            let Ok(plain) = crypt.decrypt(&purpose, column.as_bytes(), &sealed) else {
                out.unreadable += 1;
                continue;
            };
            let resealed = crypt
                .encrypt(&purpose, column.as_bytes(), &plain)
                .map_err(|e| crate::CoreError::Internal(e.to_string()))?;
            let (sql, values) = build(
                db.backend,
                Query::update()
                    .table(t.clone())
                    .value(c.clone(), resealed)
                    .and_where(Expr::col(id.clone()).eq(row_id))
                    .to_owned(),
            );
            bind_values(sqlx::query(&sql), values)
                .execute(&db.pool)
                .await?;
            out.resealed += 1;
        }
    }
    Ok(out)
}

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use chacha20poly1305::aead::{Aead, Generate, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use secrecy::{ExposeSecret, SecretBox};

/// The format version written into every sealed value and signature.
const VERSION: u8 = 1;
const NONCE_LEN: usize = 24;
const TAG_LEN: usize = 32;

/// What went wrong sealing, opening or checking.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CryptError {
    /// The value is not one this crypt wrote: malformed, another version, a
    /// key id it does not hold, a wrong purpose or AAD, or tampered with.
    #[error("the value cannot be opened")]
    Invalid,
    /// A token whose expiry has passed.
    #[error("the token has expired")]
    Expired,
    /// A key string that is not an id and 32 base64 bytes.
    #[error("not a key: {0}")]
    BadKey(String),
}

/// A 32-byte master secret with its id.
pub struct MasterKey {
    id: u8,
    secret: SecretBox<[u8; 32]>,
}

impl MasterKey {
    /// A key from `id:<base64>` (standard or url-safe base64, 32 bytes). A bare
    /// base64 string takes id 1.
    pub fn parse(text: &str) -> Result<Self, CryptError> {
        let text = text.trim();
        let (id, body) = match text.split_once(':') {
            Some((id, body)) => (
                id.trim()
                    .parse::<u8>()
                    .map_err(|_| CryptError::BadKey(format!("id `{id}` is not 0 to 255")))?,
                body.trim(),
            ),
            None => (1, text),
        };
        let bytes = decode_any(body).ok_or_else(|| CryptError::BadKey("not base64".into()))?;
        let secret: [u8; 32] = bytes
            .try_into()
            .map_err(|_| CryptError::BadKey("not 32 bytes".into()))?;
        Ok(Self {
            id,
            secret: SecretBox::new(Box::new(secret)),
        })
    }

    /// A fresh random key with `id`.
    pub fn generate(id: u8) -> Self {
        let secret: [u8; 32] = rand::random();
        Self {
            id,
            secret: SecretBox::new(Box::new(secret)),
        }
    }

    /// A key derived from arbitrary secret bytes (a legacy secret string), so a
    /// deployment that configured one keeps a stable key.
    pub fn derived(id: u8, material: &[u8]) -> Self {
        let secret = blake3::derive_key("laterite v1 master from material", material);
        Self {
            id,
            secret: SecretBox::new(Box::new(secret)),
        }
    }

    /// The key as `id:<base64>`, for writing into a configuration file.
    pub fn export(&self) -> String {
        format!("{}:{}", self.id, B64.encode(self.secret.expose_secret()))
    }

    pub fn id(&self) -> u8 {
        self.id
    }

    fn subkey(&self, context: &str) -> [u8; 32] {
        blake3::derive_key(context, self.secret.expose_secret())
    }
}

impl fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MasterKey")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

fn decode_any(text: &str) -> Option<Vec<u8>> {
    B64.decode(text)
        .ok()
        .or_else(|| base64::engine::general_purpose::STANDARD.decode(text).ok())
        .or_else(|| {
            base64::engine::general_purpose::STANDARD_NO_PAD
                .decode(text)
                .ok()
        })
        .or_else(|| base64::engine::general_purpose::URL_SAFE.decode(text).ok())
}

/// The application's keys: the current one writes, every one reads.
#[derive(Debug)]
pub struct Crypt {
    current: MasterKey,
    previous: Vec<MasterKey>,
}

impl Crypt {
    /// A crypt over `current` and the keys still readable.
    pub fn new(current: MasterKey, previous: Vec<MasterKey>) -> Self {
        Self { current, previous }
    }

    /// A crypt for one process: a random key nobody else holds, for tests and
    /// for a development run with nothing configured.
    pub fn ephemeral() -> Self {
        Self::new(MasterKey::generate(1), Vec::new())
    }

    /// The configured keys: `key` as `id:<base64>` and `previous` likewise.
    pub fn from_config(key: &str, previous: &[String]) -> Result<Self, CryptError> {
        let current = MasterKey::parse(key)?;
        let previous = previous
            .iter()
            .map(|k| MasterKey::parse(k))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self::new(current, previous))
    }

    /// The id the next write uses.
    pub fn current_id(&self) -> u8 {
        self.current.id
    }

    /// The id of the key a sealed value or signature was written under, when
    /// it is well-formed.
    pub fn key_id_of(&self, sealed: &str) -> Option<u8> {
        let bytes = B64.decode(sealed.trim()).ok()?;
        (bytes.len() >= 2 && bytes[0] == VERSION).then_some(bytes[1])
    }

    fn key(&self, id: u8) -> Option<&MasterKey> {
        if self.current.id == id {
            return Some(&self.current);
        }
        self.previous.iter().find(|k| k.id == id)
    }

    /// Seals `plain` for `purpose`, bound to `aad` (a table, column and row,
    /// or whatever must match on open). Base64url text.
    pub fn encrypt(&self, purpose: &str, aad: &[u8], plain: &[u8]) -> Result<String, CryptError> {
        let sub = self.current.subkey(&format!("laterite v1 enc:{purpose}"));
        let cipher = XChaCha20Poly1305::new_from_slice(&sub).map_err(|_| CryptError::Invalid)?;
        let nonce = XNonce::generate();
        let sealed = cipher
            .encrypt(&nonce, Payload { msg: plain, aad })
            .map_err(|_| CryptError::Invalid)?;
        let mut out = Vec::with_capacity(2 + NONCE_LEN + sealed.len());
        out.push(VERSION);
        out.push(self.current.id);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&sealed);
        Ok(B64.encode(out))
    }

    /// Opens a value [`encrypt`](Self::encrypt) sealed under any key the crypt
    /// holds, for the same purpose and AAD.
    pub fn decrypt(&self, purpose: &str, aad: &[u8], sealed: &str) -> Result<Vec<u8>, CryptError> {
        let bytes = B64.decode(sealed.trim()).map_err(|_| CryptError::Invalid)?;
        if bytes.len() < 2 + NONCE_LEN + 16 || bytes[0] != VERSION {
            return Err(CryptError::Invalid);
        }
        let key = self.key(bytes[1]).ok_or(CryptError::Invalid)?;
        let sub = key.subkey(&format!("laterite v1 enc:{purpose}"));
        let cipher = XChaCha20Poly1305::new_from_slice(&sub).map_err(|_| CryptError::Invalid)?;
        let nonce = XNonce::try_from(&bytes[2..2 + NONCE_LEN]).map_err(|_| CryptError::Invalid)?;
        cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: &bytes[2 + NONCE_LEN..],
                    aad,
                },
            )
            .map_err(|_| CryptError::Invalid)
    }

    /// Signs `message` for `purpose`. Base64url text of the version, key id
    /// and tag.
    pub fn sign(&self, purpose: &str, message: &[u8]) -> String {
        let sub = self.current.subkey(&format!("laterite v1 mac:{purpose}"));
        let tag = blake3::keyed_hash(&sub, message);
        let mut out = Vec::with_capacity(2 + TAG_LEN);
        out.push(VERSION);
        out.push(self.current.id);
        out.extend_from_slice(tag.as_bytes());
        B64.encode(out)
    }

    /// Whether `signature` is [`sign`](Self::sign) of `message` under any key
    /// the crypt holds, compared in constant time.
    pub fn verify(&self, purpose: &str, message: &[u8], signature: &str) -> bool {
        let Ok(bytes) = B64.decode(signature.trim()) else {
            return false;
        };
        if bytes.len() != 2 + TAG_LEN || bytes[0] != VERSION {
            return false;
        }
        let Some(key) = self.key(bytes[1]) else {
            return false;
        };
        let sub = key.subkey(&format!("laterite v1 mac:{purpose}"));
        let expected = blake3::keyed_hash(&sub, message);
        let mut given = [0u8; TAG_LEN];
        given.copy_from_slice(&bytes[2..]);
        expected == blake3::Hash::from(given)
    }

    /// A signed token carrying `payload` and an expiry `ttl` from now.
    pub fn token(&self, purpose: &str, payload: &[u8], ttl: Duration) -> String {
        let expires = now_secs().saturating_add(ttl.as_secs());
        let body = token_body(expires, payload);
        let signature = self.sign(purpose, &body);
        format!("{}.{}", B64.encode(body), signature)
    }

    /// The payload of a live token minted by [`token`](Self::token) for the
    /// same purpose.
    pub fn check_token(&self, purpose: &str, token: &str) -> Result<Vec<u8>, CryptError> {
        let (body, signature) = token.trim().split_once('.').ok_or(CryptError::Invalid)?;
        let body = B64.decode(body).map_err(|_| CryptError::Invalid)?;
        if !self.verify(purpose, &body, signature) || body.len() < 8 {
            return Err(CryptError::Invalid);
        }
        let expires = u64::from_be_bytes(body[..8].try_into().map_err(|_| CryptError::Invalid)?);
        if expires < now_secs() {
            return Err(CryptError::Expired);
        }
        Ok(body[8..].to_vec())
    }

    /// A keyed digest of `plain` for looking a sealed value up by equality
    /// (a blind index), under its own subkey. Base64url text.
    pub fn blind_index(&self, purpose: &str, plain: &[u8]) -> String {
        let sub = self.current.subkey(&format!("laterite v1 index:{purpose}"));
        B64.encode(blake3::keyed_hash(&sub, plain).as_bytes())
    }
}

fn token_body(expires: u64, payload: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(8 + payload.len());
    body.extend_from_slice(&expires.to_be_bytes());
    body.extend_from_slice(payload);
    body
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_round_trips_and_opens_under_its_purpose_and_aad_alone() {
        let crypt = Crypt::ephemeral();
        let sealed = crypt.encrypt("secret", b"t.c.1", b"hunter2").unwrap();
        assert_eq!(
            crypt.decrypt("secret", b"t.c.1", &sealed).unwrap(),
            b"hunter2"
        );
        assert_eq!(
            crypt.decrypt("other", b"t.c.1", &sealed),
            Err(CryptError::Invalid)
        );
        assert_eq!(
            crypt.decrypt("secret", b"t.c.2", &sealed),
            Err(CryptError::Invalid)
        );
        let mut bent = sealed.clone();
        bent.replace_range(10..11, if &sealed[10..11] == "A" { "B" } else { "A" });
        assert_eq!(
            crypt.decrypt("secret", b"t.c.1", &bent),
            Err(CryptError::Invalid)
        );
        assert_ne!(
            crypt.encrypt("secret", b"t.c.1", b"hunter2").unwrap(),
            sealed
        );
    }

    #[test]
    fn a_previous_key_still_opens_and_signs_are_checked_by_id() {
        let old = MasterKey::generate(1);
        let old_export = old.export();
        let older = Crypt::new(old, Vec::new());
        let sealed = older.encrypt("secret", b"", b"x").unwrap();
        let sig = older.sign("url", b"m");
        let rotated = Crypt::new(
            MasterKey::generate(2),
            vec![MasterKey::parse(&old_export).unwrap()],
        );
        assert_eq!(rotated.decrypt("secret", b"", &sealed).unwrap(), b"x");
        assert!(rotated.verify("url", b"m", &sig));
        assert_eq!(rotated.current_id(), 2);
        let fresh = Crypt::new(MasterKey::generate(2), Vec::new());
        assert_eq!(
            fresh.decrypt("secret", b"", &sealed),
            Err(CryptError::Invalid)
        );
        assert!(!fresh.verify("url", b"m", &sig));
    }

    #[test]
    fn a_signature_binds_message_and_purpose() {
        let crypt = Crypt::ephemeral();
        let sig = crypt.sign("url", b"media:abc:99");
        assert!(crypt.verify("url", b"media:abc:99", &sig));
        assert!(!crypt.verify("url", b"media:abc:98", &sig));
        assert!(!crypt.verify("token", b"media:abc:99", &sig));
        assert!(!crypt.verify("url", b"media:abc:99", "nope"));
    }

    #[test]
    fn a_token_carries_its_payload_until_it_expires() {
        let crypt = Crypt::ephemeral();
        let token = crypt.token("reset", b"user:7", Duration::from_secs(60));
        assert_eq!(crypt.check_token("reset", &token).unwrap(), b"user:7");
        assert_eq!(
            crypt.check_token("invite", &token),
            Err(CryptError::Invalid)
        );
        let dead = crypt.token("reset", b"user:7", Duration::from_secs(0));
        std::thread::sleep(Duration::from_millis(1100));
        assert_eq!(crypt.check_token("reset", &dead), Err(CryptError::Expired));
    }

    #[test]
    fn keys_parse_in_either_alphabet_and_export_with_their_id() {
        let key = MasterKey::generate(3);
        let text = key.export();
        assert!(text.starts_with("3:"));
        let back = MasterKey::parse(&text).unwrap();
        assert_eq!(back.id(), 3);
        assert_eq!(back.subkey("x"), key.subkey("x"));
        let bare = base64::engine::general_purpose::STANDARD.encode([7u8; 32]);
        assert_eq!(MasterKey::parse(&bare).unwrap().id(), 1);
        assert!(matches!(
            MasterKey::parse("1:short"),
            Err(CryptError::BadKey(_))
        ));
        assert!(matches!(
            MasterKey::parse("300:AAAA"),
            Err(CryptError::BadKey(_))
        ));
        let a = MasterKey::derived(0, b"legacy secret");
        let b = MasterKey::derived(0, b"legacy secret");
        assert_eq!(a.subkey("x"), b.subkey("x"));
        let index = Crypt::ephemeral();
        assert_eq!(
            index.blind_index("pan", b"1234"),
            index.blind_index("pan", b"1234")
        );
        assert_ne!(
            index.blind_index("pan", b"1234"),
            index.blind_index("pan", b"1235")
        );
    }
}
