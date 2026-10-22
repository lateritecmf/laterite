# Database

`[database]` names the connection.

```toml
[database]
url = "postgres://localhost/acme_dev"
max_connections = 10             # optional
acquire_timeout_secs = 5         # optional
```

Key | Description
--- | ---
`url` | The connection URL. Required.
`max_connections` | The pool ceiling. Default `10`.
`acquire_timeout_secs` | Seconds to wait for a pooled connection. Default `5`.

## Choose the driver

Database | URL | Feature
--- | --- | ---
PostgreSQL | `postgres://user:pass@host/acme` | `postgres`
MySQL / MariaDB | `mysql://user:pass@host/acme` | `mysql`
SQLite | `sqlite://storage/database.db?mode=rwc` | `sqlite`

```toml
[dependencies]
lateritecmf = { version = "0.9", features = ["postgres"] }
```

## Keep the URL out of version control

```toml
# config/local.toml, git-ignored
[database]
url = "postgres://localhost/acme_dev"
```

```bash
LAT__DATABASE__URL="postgres://acme:secret@db.internal/acme" ./acme
```

## Give `lat` the URL

```bash
lat admin list --database-url postgres://localhost/acme_dev
DATABASE_URL=postgres://localhost/acme_dev lat admin list
```

Source | Order
--- | ---
`--database-url` | First.
`DATABASE_URL` | Second.
`database.url` of the application found from the current directory upward | Last.
