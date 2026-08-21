#!/usr/bin/env bash
# Generates each crate's README.md from its crate-level doc comment (the `//!`
# block in lib.rs, or main.rs for the binary), wrapped in the shared README.tpl,
# using cargo-readme. Run after editing a crate's doc comment. CI checks that the
# committed READMEs match this output, so they never drift from the source.
#
# Requires: cargo install cargo-readme
set -euo pipefail

cd "$(dirname "$0")/.."
root="$(pwd)"
tpl="$root/README.tpl"

if ! cargo readme --version >/dev/null 2>&1; then
  echo "cargo-readme is not installed. Run: cargo install cargo-readme" >&2
  exit 1
fi

gen() {
  local crate="$1" input="$2"
  ( cd "crates/$crate" && cargo readme --template "$tpl" --input "$input" --output README.md )
  echo "generated crates/$crate/README.md"
}

gen core src/lib.rs
gen auth src/lib.rs
gen admin src/lib.rs
gen web src/lib.rs
gen cli src/main.rs
