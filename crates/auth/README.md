<!-- Generated from the crate's doc comment. Do not edit by hand: edit the //!
block in the crate source and run scripts/gen-readmes.sh. -->
# laterite-auth

Laterite auth: backend user authentication and authorization.

Provides the operator-facing security primitives the admin surface is
built on: Argon2id password hashing, opaque server-side sessions, a
role-based permission model over dotted permission strings, brute-force
throttling, and an append-only access log. "Backend users" are the
operators of the admin, kept distinct from any application's end users.

This crate is HTTP-agnostic on purpose. It exposes an [`AuthService`] with
plain async methods (`authenticate`, `verify_session`, `logout`) plus an
[`AuthenticatedUser`] identity; the admin crate wraps these in Axum
extractors, cookie handling, and the rendered login screen.

## Part of Laterite

This crate is part of [Laterite](https://github.com/lateritecmf/laterite), a
content management framework for Rust. See the repository for the guide, the
full crate set, and the `lat` command-line tool.

## License

Licensed under either the MIT license or the Apache License 2.0, at your option.
