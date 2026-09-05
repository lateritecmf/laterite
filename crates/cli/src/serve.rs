//! `lat serve`: run the application found from the current directory upward.
//!
//! A Laterite application is a standalone binary that serves itself, so this is
//! a thin convenience over `cargo run`: it translates `--host`/`--port`/`--listen`
//! into the `<PREFIX>__SERVER__LISTEN` override the config layer already honours,
//! under the prefix the app declares, so a quick bind-address change needs no
//! config edit. With no flags it runs the app on its configured address.

use std::process::Command;

use anyhow::{bail, Context, Result};
use clap::Args;

use crate::project::Project;

#[derive(Args)]
pub struct ServeArgs {
    /// Bind host, overriding config (e.g. `0.0.0.0`). Combine with `--port`; the
    /// unset half is taken from the app's configured address.
    #[arg(long)]
    host: Option<String>,
    /// Bind port, overriding config (e.g. `3000`).
    #[arg(long)]
    port: Option<u16>,
    /// Full bind address `host:port`, overriding config. An alternative to
    /// `--host`/`--port`.
    #[arg(long, conflicts_with_all = ["host", "port"])]
    listen: Option<String>,
    /// Build and run in release mode (optimised, slower to compile).
    #[arg(long)]
    release: bool,
}

pub fn run(args: ServeArgs) -> Result<()> {
    let project = Project::locate()?;
    if !project.root.join("Cargo.toml").is_file() {
        bail!(
            "run `lat serve` inside a Laterite application (one created by `lat new`): \
             no Cargo.toml in {}.",
            project.root.display()
        );
    }

    let listen = resolve_listen(&args, &current_listen(&project));

    let mut cmd = Command::new("cargo");
    cmd.arg("run").current_dir(&project.root);
    if args.release {
        cmd.arg("--release");
    }
    if let Some(listen) = &listen {
        // Override under the app's own prefix, so the running app sees it.
        cmd.env(project.env_key("SERVER__LISTEN"), listen);
        println!("Overriding the bind address: {listen}");
    }

    let status = cmd
        .status()
        .context("could not run `cargo run`; is the Rust toolchain installed and on PATH?")?;
    if !status.success() {
        // Pass the child's exit code through (a signal, e.g. Ctrl-C, has no code
        // and is a normal way to stop the server, so it exits cleanly).
        std::process::exit(status.code().unwrap_or(0));
    }
    Ok(())
}

/// The bind address to override with, or `None` to leave the app's configured
/// address untouched. `--listen` wins; otherwise `--host`/`--port` override the
/// respective half of `current`, so passing only one keeps the other.
fn resolve_listen(args: &ServeArgs, current: &str) -> Option<String> {
    if let Some(listen) = &args.listen {
        return Some(listen.clone());
    }
    if args.host.is_none() && args.port.is_none() {
        return None;
    }
    let (cur_host, cur_port) = split_host_port(current);
    let host = args.host.clone().unwrap_or(cur_host);
    let port = args.port.map(|p| p.to_string()).unwrap_or(cur_port);
    Some(format!("{host}:{port}"))
}

/// Splits a `host:port` bind string at the last colon (so a bracketed IPv6 host
/// stays intact), falling back to port `8080` when there is no colon.
fn split_host_port(listen: &str) -> (String, String) {
    match listen.rsplit_once(':') {
        Some((host, port)) => (host.to_string(), port.to_string()),
        None => (listen.to_string(), "8080".to_string()),
    }
}

/// The application's currently configured bind address, read from its config so
/// a partial `--host`/`--port` override can fill the other half. Best-effort:
/// falls back to the framework default if the config cannot be read.
fn current_listen(project: &Project) -> String {
    #[derive(serde::Deserialize)]
    struct ServeConfig {
        server: laterite_core::config::ServerConfig,
    }
    project
        .load::<ServeConfig>()
        .map(|c| c.server.listen)
        .unwrap_or_else(|_| "127.0.0.1:8080".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(host: Option<&str>, port: Option<u16>, listen: Option<&str>) -> ServeArgs {
        ServeArgs {
            host: host.map(str::to_string),
            port,
            listen: listen.map(str::to_string),
            release: false,
        }
    }

    #[test]
    fn no_flags_leaves_the_configured_address() {
        assert_eq!(
            resolve_listen(&args(None, None, None), "127.0.0.1:8080"),
            None
        );
    }

    #[test]
    fn listen_flag_wins_verbatim() {
        assert_eq!(
            resolve_listen(&args(None, None, Some("0.0.0.0:3000")), "127.0.0.1:8080"),
            Some("0.0.0.0:3000".to_string())
        );
    }

    #[test]
    fn host_or_port_fills_the_other_half_from_config() {
        // Only a port: keep the configured host.
        assert_eq!(
            resolve_listen(&args(None, Some(3000), None), "0.0.0.0:8080"),
            Some("0.0.0.0:3000".to_string())
        );
        // Only a host: keep the configured port.
        assert_eq!(
            resolve_listen(&args(Some("0.0.0.0"), None, None), "127.0.0.1:9000"),
            Some("0.0.0.0:9000".to_string())
        );
        // Both: compose them.
        assert_eq!(
            resolve_listen(&args(Some("0.0.0.0"), Some(3000), None), "127.0.0.1:8080"),
            Some("0.0.0.0:3000".to_string())
        );
    }

    #[test]
    fn split_keeps_bracketed_ipv6_host() {
        assert_eq!(
            split_host_port("[::1]:8080"),
            ("[::1]".to_string(), "8080".to_string())
        );
    }
}
