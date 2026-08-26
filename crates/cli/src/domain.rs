//! `lat domain`: Valet-style local wildcard domains (macOS + dnsmasq).
//!
//! Maps every host under a reserved TLD (default `.test`) to loopback, so an app
//! served on `127.0.0.1` is reachable as `http://acme.test:PORT` instead of a
//! bare IP. This is the DNS layer: dnsmasq answers `*.<tld>` with `127.0.0.1`,
//! and a macOS resolver entry points the OS at dnsmasq for that TLD. It is set up
//! once and every future app and subdomain works with no extra step.
//!
//! Dropping the port and serving HTTPS (so `https://acme.test` works with no
//! port) is a separate, later concern: a reverse proxy that reuses this DNS. The
//! privileged steps (the `/etc/resolver` entry and starting dnsmasq on port 53)
//! run through `sudo`, which prompts in the terminal; `--dry-run` prints the plan
//! without touching anything, and `teardown` reverses it.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};

#[derive(Args)]
pub struct DomainArgs {
    #[command(subcommand)]
    command: DomainCommand,
}

#[derive(Subcommand)]
enum DomainCommand {
    /// Route *.<tld> to 127.0.0.1 via dnsmasq and a macOS resolver entry.
    Setup(ActionArgs),
    /// Report the current local-domain DNS state.
    Status(TldArgs),
    /// Undo the wildcard DNS configuration.
    Teardown(ActionArgs),
}

#[derive(Args)]
struct TldArgs {
    /// The top-level domain routed to loopback (a reserved `.test` by default).
    #[arg(long, default_value = "test")]
    tld: String,
}

#[derive(Args)]
struct ActionArgs {
    /// The top-level domain routed to loopback (a reserved `.test` by default).
    #[arg(long, default_value = "test")]
    tld: String,
    /// Print what would change without modifying anything.
    #[arg(long)]
    dry_run: bool,
}

pub fn run(args: DomainArgs) -> Result<()> {
    if !cfg!(target_os = "macos") {
        bail!("`lat domain` currently supports macOS (dnsmasq + /etc/resolver) only.");
    }
    match args.command {
        DomainCommand::Setup(a) => setup(&a.tld, a.dry_run),
        DomainCommand::Status(a) => status(&a.tld),
        DomainCommand::Teardown(a) => teardown(&a.tld, a.dry_run),
    }
}

/// The dnsmasq directive answering the whole TLD (and its subdomains) with
/// loopback.
fn wildcard_line(tld: &str) -> String {
    format!("address=/{tld}/127.0.0.1")
}

/// The macOS resolver body pointing the TLD at the local dnsmasq.
fn resolver_body() -> &'static str {
    "nameserver 127.0.0.1\n"
}

fn resolver_path(tld: &str) -> PathBuf {
    Path::new("/etc/resolver").join(tld)
}

fn setup(tld: &str, dry_run: bool) -> Result<()> {
    let prefix = brew_prefix()?;
    if !dnsmasq_installed(&prefix) {
        println!(
            "dnsmasq is not installed. Install it first:\n\n    brew install dnsmasq\n\n\
             then re-run `lat domain setup`."
        );
        return Ok(());
    }

    let conf = prefix.join("etc/dnsmasq.conf");
    let line = wildcard_line(tld);

    // 1. dnsmasq wildcard. Skip when the TLD is already answered: our own line,
    // one placed by another tool (Valet keeps it in dnsmasq.d, often dotted), or
    // simply a live resolution. This keeps setup from duplicating a working rule.
    let conf_changed = if configured_anywhere(&prefix, tld) || resolves(tld) == Some(true) {
        println!("Wildcard for .{tld} is already answered by dnsmasq; leaving it as-is.");
        false
    } else if dry_run {
        println!("would add to {}:  {line}", conf.display());
        true
    } else {
        let existing = std::fs::read_to_string(&conf).unwrap_or_default();
        if let Some(updated) = ensure_line(&existing, &line) {
            std::fs::write(&conf, updated)
                .with_context(|| format!("writing {}", conf.display()))?;
        }
        println!("Added to {}:  {line}", conf.display());
        true
    };

    // 2. macOS resolver entry: under /etc, so it needs admin rights.
    let resolver = resolver_path(tld);
    let resolver_ok = resolver_has_loopback(&resolver);
    if resolver_ok {
        println!("Already present:  {}", resolver.display());
    } else if dry_run {
        println!(
            "would create {} with `nameserver 127.0.0.1` (via sudo)",
            resolver.display()
        );
    } else {
        println!("Creating {} (needs admin rights)...", resolver.display());
        write_resolver_with_sudo(&resolver)?;
    }

    // 3. (Re)start dnsmasq (binds port 53, so needs admin rights) when anything
    // changed.
    if dry_run {
        println!("would run:  sudo brew services restart dnsmasq");
    } else if conf_changed || !resolver_ok {
        println!("Restarting dnsmasq (needs admin rights)...");
        sudo(&["brew", "services", "restart", "dnsmasq"])?;
    } else {
        println!("dnsmasq already configured; nothing to restart.");
    }

    if !dry_run {
        println!("\nDone. Verify with:  lat domain status --tld {tld}");
        println!("Then serve an app and open  http://acme.{tld}:<port>/admin");
    }
    Ok(())
}

fn status(tld: &str) -> Result<()> {
    let prefix = brew_prefix().ok();
    let installed = prefix.as_deref().map(dnsmasq_installed).unwrap_or(false);
    let resolver_present = resolver_has_loopback(&resolver_path(tld));
    let resolving = resolves(tld);

    // Resolution is the source of truth: the wildcard may live in the main conf,
    // a dnsmasq.d include, or another tool's config (Valet), so a live query is
    // the reliable signal rather than guessing which file holds it.
    println!("Local domains (.{tld}):\n");
    mark(
        "Homebrew present",
        prefix.is_some(),
        "install from https://brew.sh",
    );
    mark("dnsmasq installed", installed, "brew install dnsmasq");
    mark(
        &format!("resolver entry {}", resolver_path(tld).display()),
        resolver_present,
        "lat domain setup",
    );
    match resolving {
        Some(true) => mark("resolves to 127.0.0.1", true, ""),
        Some(false) => mark("resolves to 127.0.0.1", false, "lat domain setup"),
        None => println!("  ?   resolves to 127.0.0.1   (install `dig` to check)"),
    }
    if resolving == Some(true) {
        println!("\nReady: *.{tld} resolves to loopback (serve an app and open http://acme.{tld}:<port>).");
    }
    Ok(())
}

fn teardown(tld: &str, dry_run: bool) -> Result<()> {
    let prefix = brew_prefix()?;
    let conf = prefix.join("etc/dnsmasq.conf");
    let line = wildcard_line(tld);
    let existing = std::fs::read_to_string(&conf).unwrap_or_default();

    // Ownership rule: lat only undoes what it set up. If its own line is not in
    // the main conf, another tool (e.g. Valet, which keeps its wildcard and the
    // resolver entry elsewhere) owns this TLD, so touch nothing.
    let Some(updated) = remove_line(&existing, &line) else {
        println!(
            "lat did not configure .{tld} (no `{line}` in {}).",
            conf.display()
        );
        if resolves(tld) == Some(true) {
            println!(
                "It still resolves, so another tool (e.g. Valet) provides it; leaving {} and dnsmasq untouched.",
                resolver_path(tld).display()
            );
        }
        return Ok(());
    };

    let resolver = resolver_path(tld);
    let rp = resolver.to_string_lossy().into_owned();
    if dry_run {
        println!("would remove from {}:  {line}", conf.display());
        if resolver.exists() {
            println!("would run:  sudo rm {rp}");
        }
        println!("would run:  sudo brew services restart dnsmasq");
        return Ok(());
    }

    std::fs::write(&conf, updated).with_context(|| format!("writing {}", conf.display()))?;
    println!("Removed from {}:  {line}", conf.display());
    if resolver.exists() {
        println!("Removing {rp} (needs admin rights)...");
        sudo(&["rm", "-f", &rp])?;
    }
    println!("Restarting dnsmasq (needs admin rights)...");
    sudo(&["brew", "services", "restart", "dnsmasq"])?;
    Ok(())
}

/// Adds `line` to `content` if absent, returning the new content, or `None` when
/// it is already present (so a caller can skip a write and report "unchanged").
fn ensure_line(content: &str, line: &str) -> Option<String> {
    if content.lines().any(|l| l.trim() == line) {
        return None;
    }
    let mut updated = content.to_string();
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(line);
    updated.push('\n');
    Some(updated)
}

/// Removes exactly `line` from `content`, returning the new content, or `None`
/// when it was not present. Other directives are untouched.
fn remove_line(content: &str, line: &str) -> Option<String> {
    if !content.lines().any(|l| l.trim() == line) {
        return None;
    }
    let kept: Vec<&str> = content.lines().filter(|l| l.trim() != line).collect();
    let mut updated = kept.join("\n");
    if !updated.is_empty() {
        updated.push('\n');
    }
    Some(updated)
}

/// The Homebrew prefix (`/opt/homebrew` or `/usr/local`).
fn brew_prefix() -> Result<PathBuf> {
    let out = Command::new("brew")
        .arg("--prefix")
        .output()
        .context("could not run `brew`; install Homebrew (https://brew.sh) first")?;
    if !out.status.success() {
        bail!("`brew --prefix` failed; is Homebrew installed?");
    }
    Ok(PathBuf::from(
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
    ))
}

fn dnsmasq_installed(prefix: &Path) -> bool {
    prefix.join("sbin/dnsmasq").exists() || prefix.join("bin/dnsmasq").exists()
}

/// A dnsmasq `address` directive answering the TLD, in either the plain
/// (`address=/test/...`) or dotted (`address=/.test/...`) form other tools use.
fn line_matches(line: &str, tld: &str) -> bool {
    let l = line.trim();
    l == format!("address=/{tld}/127.0.0.1") || l == format!("address=/.{tld}/127.0.0.1")
}

fn file_has_wildcard(path: &Path, tld: &str) -> bool {
    std::fs::read_to_string(path)
        .map(|c| c.lines().any(|l| line_matches(l, tld)))
        .unwrap_or(false)
}

/// Whether a file under `etc/dnsmasq.d` answers the TLD. This is where tools like
/// Laravel Valet keep their wildcard, so we neither duplicate nor remove it.
fn configured_in_dnsmasqd(prefix: &Path, tld: &str) -> bool {
    match std::fs::read_dir(prefix.join("etc/dnsmasq.d")) {
        Ok(entries) => entries.flatten().any(|e| file_has_wildcard(&e.path(), tld)),
        Err(_) => false,
    }
}

/// Whether any dnsmasq config answers the TLD: the main conf or a dnsmasq.d
/// include. Used with a live resolution check to avoid adding a duplicate rule.
fn configured_anywhere(prefix: &Path, tld: &str) -> bool {
    file_has_wildcard(&prefix.join("etc/dnsmasq.conf"), tld) || configured_in_dnsmasqd(prefix, tld)
}

fn resolver_has_loopback(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .map(|c| c.contains("127.0.0.1"))
        .unwrap_or(false)
}

/// Whether dnsmasq answers a probe under the TLD with loopback. `None` when
/// `dig` is unavailable to ask.
fn resolves(tld: &str) -> Option<bool> {
    let probe = format!("probe.{tld}");
    let out = Command::new("dig")
        .args(["+short", &probe, "@127.0.0.1"])
        .output()
        .ok()?;
    if !out.status.success() {
        return Some(false);
    }
    Some(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .any(|l| l.trim() == "127.0.0.1"),
    )
}

fn sudo(args: &[&str]) -> Result<()> {
    let status = Command::new("sudo")
        .args(args)
        .status()
        .with_context(|| format!("could not run `sudo {}`", args.join(" ")))?;
    if !status.success() {
        bail!("`sudo {}` did not succeed", args.join(" "));
    }
    Ok(())
}

/// Writes the resolver file through `sudo tee`, since `/etc/resolver` is
/// root-owned. The content is piped in, never passed as an argument.
fn write_resolver_with_sudo(path: &Path) -> Result<()> {
    sudo(&["mkdir", "-p", "/etc/resolver"])?;
    let mut child = Command::new("sudo")
        .arg("tee")
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .context("could not run `sudo tee`")?;
    child
        .stdin
        .take()
        .expect("child stdin was piped")
        .write_all(resolver_body().as_bytes())?;
    if !child.wait()?.success() {
        bail!("could not write {}", path.display());
    }
    Ok(())
}

fn mark(label: &str, ok: bool, hint: &str) {
    let sym = if ok { "OK " } else { "-- " };
    if ok || hint.is_empty() {
        println!("  {sym} {label}");
    } else {
        println!("  {sym} {label}   (try: {hint})");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_line_points_the_tld_at_loopback() {
        assert_eq!(wildcard_line("test"), "address=/test/127.0.0.1");
        assert_eq!(wildcard_line("localhost"), "address=/localhost/127.0.0.1");
    }

    #[test]
    fn line_matches_accepts_plain_and_dotted_forms() {
        // Our plain form and the dotted form other tools (e.g. Valet) use.
        assert!(line_matches("address=/test/127.0.0.1", "test"));
        assert!(line_matches("  address=/.test/127.0.0.1  ", "test"));
        // A different TLD or target does not match.
        assert!(!line_matches("address=/dev/127.0.0.1", "test"));
        assert!(!line_matches("address=/test/10.0.0.1", "test"));
    }

    #[test]
    fn ensure_line_adds_once_and_is_idempotent() {
        let line = wildcard_line("test");
        let added = ensure_line("port=53\n", &line).unwrap();
        assert!(added.contains(&line));
        assert!(added.starts_with("port=53\n"));
        assert!(added.ends_with('\n'));
        // Already present: no change.
        assert!(ensure_line(&added, &line).is_none());
        // Adds a trailing newline when the file lacked one.
        assert!(ensure_line("port=53", &line).unwrap().ends_with('\n'));
    }

    #[test]
    fn remove_line_takes_only_that_line() {
        let line = wildcard_line("test");
        let content = format!("port=53\n{line}\naddress=/other/1.2.3.4\n");
        let removed = remove_line(&content, &line).unwrap();
        assert!(!removed.contains(&line));
        assert!(removed.contains("port=53"));
        assert!(removed.contains("address=/other/1.2.3.4"));
        // Not present: None.
        assert!(remove_line("port=53\n", &line).is_none());
    }
}
