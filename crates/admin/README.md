<!-- Generated from the crate's doc comment. Do not edit by hand: edit the //!
block in the crate source and run scripts/gen-readmes.sh. -->
# laterite-admin

Laterite admin: the operator-facing web surface.

An Axum router mounted under a configurable path (default `/admin`, set by
[`AdminConfig::path`]): a login screen and session cookie verified against
`laterite-auth`, and descriptor-driven screens.

Screens are **resources**: a module declares a [`Resource`] (a
[`list::ListConfig`], optionally a [`form::FormConfig`], a base path, and a
menu label), and the framework mounts the list, create, and edit routes and
adds it to the menu. This is the extension point that lets an application
contribute its own admin screens. The framework's own screens (users, roles)
are just built-in resources.

## Part of Laterite

This crate is part of [Laterite](https://github.com/lateritecmf/laterite), a
content management framework for Rust. See the repository for the guide, the
full crate set, and the `lat` command-line tool.

## License

Licensed under either the MIT license or the Apache License 2.0, at your option.
