# Development Workflow

Two local conveniences: a `.test` domain per application and a rebuild that
keeps the browser connected.

## Use a local domain

`lat domain` routes every host under a reserved `.test` TLD to loopback. macOS,
built on `dnsmasq`.

```bash
brew install dnsmasq
lat domain setup
```

`setup` writes a `dnsmasq` wildcard rule and `/etc/resolver/test`, prompting
for `sudo`. It is idempotent.

Flag | Description
--- | ---
`--dry-run` | Print the plan and change nothing.
`--tld <name>` | Use a different reserved TLD.

### Open an application by name

```bash
lat serve --port 8080
# http://acme.test:8080/admin
```

Set `app.url` to match:

```toml
[app]
url = "http://acme.test:8080"
```

The port stays in the URL. HTTPS on the bare name is a reverse proxy's job.

### Status and teardown

```bash
lat domain status
lat domain teardown
```

`teardown` removes only what `lat domain` created. A `.test` managed by another
tool is detected: `setup` adds nothing and `teardown` leaves it alone.

## Reload on change

`systemfd` holds the port and `watchexec` rebuilds on change; the browser
reconnects after every edit.

```sh
cargo install systemfd
brew install watchexec   # or cargo install watchexec-cli
```

### Reuse the passed socket

Add [`listenfd`](https://crates.io/crates/listenfd) and take the socket in
`main`:

```rust
use listenfd::ListenFd;
use tokio::net::TcpListener;

let listener = match ListenFd::from_env().take_tcp_listener(0)? {
    Some(std_listener) => {
        std_listener.set_nonblocking(true)?;
        TcpListener::from_std(std_listener)?
    }
    None => TcpListener::bind("127.0.0.1:8080").await?,
};
axum::serve(listener, app).await?;
```

`None` is the normal path: `cargo run` and production bind directly.

### Run the loop

```sh
systemfd --no-pid -s http::8080 -- \
    watchexec -r -e rs,html,css,toml -- \
    cargo run -p acme
```

`.html` and `.css` are watched too: templates and the stylesheet are compiled
in. As a `justfile` recipe:

```just
dev:
    systemfd --no-pid -s http::8080 -- \
        watchexec -r -e rs,html,css,toml -- \
        cargo run -p acme
```

### See errors as you type

```sh
cargo install bacon   # or brew install bacon
bacon clippy
```
