# Upgrading

Moving an application to a newer framework version.

## Read the changelog

The crates release together under one version. Read the version's section of
[`CHANGELOG.md`](https://github.com/lateritecmf/laterite/blob/main/CHANGELOG.md)
before bumping.

Bump | Before 1.0
--- | ---
Patch, `0.9.19` to `0.9.20` | Additive or a fix. No code changes.
Minor, `0.9` to `0.10` | May break. The section lists what moved.

## Bump the requirements

Every `laterite-*` and `lateritecmf` requirement moves together.

```toml
# Cargo.toml
[workspace.dependencies]
lateritecmf = { version = "0.10", features = ["postgres"] }
laterite-core = "0.10"
laterite-admin = "0.10"
```

```toml
# plugins/<folder>/Cargo.toml, for each plugin the application owns
[dependencies]
laterite-core = "0.10"
laterite-admin = "0.10"
```

Crate | Description
--- | ---
`lateritecmf` | The umbrella an application depends on.
`laterite-core` | Configuration, migrations, modules, queries.
`laterite-admin` | The admin panel.
`laterite-auth` | Sign-in, sessions, permissions.
`laterite-media` | The media library.
`laterite-web` | Themes and public pages.
`laterite-macros` | The translation-string macros.
`laterite-cli` | The `lat` command.

```bash
cargo install laterite-cli
cargo update
cargo build --release
```

A patch needs `cargo update` alone; the requirement already admits it.

## Run the migrations

```bash
lat serve
```

Starting the application applies the framework's new migrations, then the
application's and the plugins'.

## Check the result

```bash
lat doctor
```
