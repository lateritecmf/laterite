#!/bin/sh
#
# check-rustdoc.sh: every public item in every published crate carries a doc
# comment, and every link inside one resolves.
#
# The API reference is rustdoc, built by docs.rs from the `///` comments alone,
# so an item without one is a page with a signature and nothing else. Each
# library crate carries `#![warn(missing_docs)]`, which lists each undocumented
# public item as a compiler warning; building the documentation with warnings
# denied turns that list, and any link to a private or missing item, into a
# failure.
#
# Runs in CI's lint job and in the local verification loop beside clippy.
#
# Usage: sh scripts/check-rustdoc.sh   (from the repository root)

set -eu

RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --quiet "$@"
echo "rustdoc: every public item is documented and every link resolves"
