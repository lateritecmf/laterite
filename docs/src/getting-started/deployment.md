# Deployment

Running an application in production: a release binary, the files beside it,
secrets from the environment, and a reverse proxy in front.

## Build the binary

```bash
cargo build --release
# target/release/acme
```

Run it from a directory holding `config/`.

## Ship the files

Path | Description
--- | ---
`acme` | The release binary. Plugins are compiled into it; nothing of `plugins/` ships.
`config/default.toml` | The committed defaults.
`config/production.toml` | Optional. Read when `APP_ENV=production`.
`storage/` | Writable by the process. Empty on the first start.

`config/local.toml` stays on the development machine. Secrets travel as
environment variables.

## Set secrets in the environment

```bash
export APP_ENV=production
export LAT__DATABASE__URL="postgres://acme:secret@db.internal/acme"
export LAT__BACKEND__SECRET="a-long-random-string"
./acme
```

Variable | Sets
--- | ---
`APP_ENV` | Which `config/<APP_ENV>.toml` layer is read. Default `development`.
`<PREFIX>__DATABASE__URL` | `database.url`.
`<PREFIX>__BACKEND__SECRET` | `backend.secret`. Set it when several instances share one database.
`<PREFIX>__SECTION__KEY` | Any other key. The prefix is `app.env_prefix`, `LAT` by default.

## Sit behind a proxy

```toml
# config/production.toml
[app]
url = "https://acme.example"

[server]
listen = "127.0.0.1:8080"

[backend]
secure_cookie = true
trusted_proxies = ["127.0.0.1/32"]
```

Key | Description
--- | ---
`app.url` | The public origin absolute links build on.
`backend.secure_cookie` | Sets the Secure flag on cookies. `true` behind HTTPS.
`backend.trusted_proxies` | CIDR ranges whose `X-Forwarded-For` is read. The proxy's own network.

```nginx
server {
    listen 443 ssl;
    server_name acme.example;

    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    }
}
```

## Gate the deploy

```bash
lat doctor
```

Check | Passes when
--- | ---
Configuration loads | Every layer parses and `database.url` is present.
Timezone is valid | `backend.timezone` is an IANA name.
`storage/` is writable | The process can write the application's `storage/`.
Database is reachable | A connection opens.
Tables exist | The framework's tables are migrated.
plugins-manifest is in sync | The list and the generated crate agree.
Descriptor files parse | Every YAML descriptor loads.

Exits non-zero on any failure. Run it from the application directory, or pass
`--database-url`.

## Recover access

```bash
lat admin create ops --email ops@acme.example --first-name Ops --generate
lat admin reset-password editor --generate
lat admin unlock editor
lat admin purge
```

Command | Description
--- | ---
`create` | A new superuser. `--generate` prints a strong password.
`reset-password` | A new password for an existing user.
`unlock` | Clears a user's failed sign-in lockout.
`purge` | Removes expired sessions and stay-signed-in credentials.

Outside the application directory, pass `--database-url` or set `DATABASE_URL`.
