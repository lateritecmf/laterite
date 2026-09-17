# Live Reload in Development

`systemfd` holds the port and `watchexec` rebuilds on change, so the browser
reconnects after every edit.

## Install

```sh
cargo install systemfd
brew install watchexec   # or cargo install watchexec-cli
```

## Reuse the passed socket

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

## Run the loop

```sh
systemfd --no-pid -s http::8080 -- \
    watchexec -r -e rs,html,css,toml -- \
    cargo run -p acme-api
```

Templates and the stylesheet are compiled in, so `.html` and `.css` are watched
too. As a `justfile` recipe:

```just
dev:
    systemfd --no-pid -s http::8080 -- \
        watchexec -r -e rs,html,css,toml -- \
        cargo run -p acme-api
```

## See errors as you type

```sh
cargo install bacon   # or brew install bacon
bacon clippy
```
