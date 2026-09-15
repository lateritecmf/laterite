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

# Every crate under crates/, discovered rather than listed, so adding one needs
# no edit here.
gen() {
  local dir="$1" crate input
  crate="$(basename "$dir")"
  if [ -f "$dir/src/lib.rs" ]; then
    input="src/lib.rs"
  elif [ -f "$dir/src/main.rs" ]; then
    input="src/main.rs"
  else
    echo "$crate has neither src/lib.rs nor src/main.rs; cannot generate a README" >&2
    exit 1
  fi
  ( cd "$dir" && cargo readme --template "$tpl" --input "$input" --output README.md )
  echo "generated $dir/README.md"
}

for dir in crates/*/; do
  [ -f "$dir/Cargo.toml" ] || continue
  gen "${dir%/}"
done
