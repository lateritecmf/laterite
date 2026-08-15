# Live Reload in Development

A Laterite application is a compiled binary, so a source change takes effect
once the binary is rebuilt and rerun. A small tool loop makes that automatic and
keeps the listening port bound across rebuilds, so the browser reconnects on its
own instead of hitting a refused connection.

Two tools cover it:

- [`systemfd`](https://github.com/mitsuhiko/systemfd) binds the listening socket
  once and passes it to each new build of your server.
- [`watchexec`](https://github.com/watchexec/watchexec) reruns the server when a
  source file changes.

```sh
cargo install systemfd
brew install watchexec   # or cargo install watchexec-cli
```

## Reuse a passed socket

For `systemfd` to hand its socket to your server, the server reuses a socket
inherited from the environment when one is present, and binds its configured
address otherwise. Add [`listenfd`](https://crates.io/crates/listenfd) and take
the socket in `main`:

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

The `None` arm is the normal path: a plain `cargo run`, and production, bind the
address directly. Only the development loop passes a socket.

## Run the loop

Wrap the run command with both tools. `systemfd` stays as the long-lived parent
that owns the socket; `watchexec` restarts the build under it:

```sh
systemfd --no-pid -s http::8080 -- \
    watchexec -r -e rs,html,css,toml -- \
    cargo run -p acme-api
```

The admin templates are compiled into the binary, so watching `.html` and `.css`
alongside `.rs` means an edit to a screen's markup or the stylesheet triggers a
rebuild and shows up on the next reconnect. A `justfile` recipe keeps the command
to hand:

```just
dev:
    systemfd --no-pid -s http::8080 -- \
        watchexec -r -e rs,html,css,toml -- \
        cargo run -p acme-api
```

## See compiler errors as you type

The reload loop shows build output in the server's terminal. For a dedicated,
navigable view of compiler and clippy errors while you edit, run
[`bacon`](https://github.com/Canop/bacon) in a second terminal:

```sh
cargo install bacon   # or brew install bacon
bacon clippy
```
