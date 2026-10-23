# Encryption and Signing

The application key seals values at rest, signs links and mints tokens with an expiry; every operation names a purpose, and a value sealed for one purpose never opens under another.

```bash
lat key generate
```

```toml
# config/local.toml
[app]
key = "1:k8xq...="            # id:<base64 of 32 bytes>; or LAT__APP__KEY
previous_keys = ["0:u9Yr...="]   # keys retired by a rotation, still read
```

```rust
let crypt = ctx.crypt();
let sealed = crypt.encrypt("api-secret", b"integrations.token.42", b"hunter2")?;
let plain = crypt.decrypt("api-secret", b"integrations.token.42", &sealed)?;
let signature = crypt.sign("export-link", b"report:7");
let ok = crypt.verify("export-link", b"report:7", &signature);
let token = crypt.token("password-reset", b"user:7", Duration::from_secs(3600));
let payload = crypt.check_token("password-reset", &token)?;
let index = crypt.blind_index("pan", b"1234-5678");
```

Method | Does
--- | ---
`encrypt(purpose, aad, plain)` | Seals `plain`, bound to `aad`; base64url text with the key id inside.
`decrypt(purpose, aad, sealed)` | Opens a value sealed under any key the application holds; `CryptError::Invalid` on a wrong purpose, AAD, key or tampering.
`sign(purpose, message)` | A signature of `message`, base64url.
`verify(purpose, message, signature)` | Constant-time check under any held key.
`token(purpose, payload, ttl)` | A signed token carrying `payload` and an expiry.
`check_token(purpose, token)` | The payload while the token lives; `CryptError::Expired` after.
`blind_index(purpose, plain)` | A keyed digest to find a sealed value by equality.

`aad` names what the value belongs to (a table, column and row); a sealed value moved to another row does not open. `ctx.sign` and `ctx.verify_signature` on a route are `sign` and `verify` for the `url` purpose.

## Set the key

Key | Description
--- | ---
`app.key` | `id:<base64>` of 32 random bytes; `lat key generate` writes one into `config/local.toml`. Unset, a key derived from `backend.secret` or the key stored at the first start is used, and a production start logs a warning.
`app.previous_keys` | Keys retired by a rotation, each `id:<base64>`, used for reading only.

Command | Does
--- | ---
`lat key generate` | Writes a new key as `app.key`. A key already set is replaced; add `--rotate` to keep it under `previous_keys`, or `--print` to print without writing.
`lat key show` | The id of the key in use and of the previous keys.

## Rotate the key

```bash
lat key generate --rotate
lat crypt rotate       # re-seals stored values under the new key
```

Remove the old key from `previous_keys` once `lat crypt rotate` has run.

## Test with a key of your own

```rust
let crypt = laterite_core::Crypt::ephemeral();
let ctx = RouteCtx::builder(db).build();   // builds on an ephemeral key
```
