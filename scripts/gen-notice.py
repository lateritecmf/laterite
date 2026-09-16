#!/usr/bin/env python3
"""Generate NOTICE: every third-party work a Laterite binary carries.

Two kinds of thing end up inside the binary and both have to be accounted for:

  Bundled assets   fonts and icons compiled in with `include_bytes!`, listed by
                   hand in ASSETS below because no manifest describes them.
  Rust crates      everything the workspace links, read from `cargo metadata`.

Licence texts are quoted once each and the works using them are listed against
that text, which is how a notice stays readable at a few hundred crates. Where a
crate ships its own LICENSE file the real text is quoted, since MIT and ISC ask
for *that* copyright line, not a generic one.

Run it and commit the result; CI fails when the committed file has drifted.
"""

from __future__ import annotations

import json
import pathlib
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
# The repository's notice. Nothing is embedded in a binary: the obligation is
# met by the distribution, which is how the rest of the ecosystem does it. A
# crate published to crates.io carries its own NOTICE in its package, and an
# application distributing a built binary generates its own from its own
# dependency tree (this script is the worked example).
OUT = ROOT / "NOTICE"

# Assets compiled into the binary, summarised for the index below. Their licence
# texts are NOT kept here: they live in `crates/admin/NOTICE`, which is where a
# dependency scanner looks, so a consumer building on Laterite picks them up
# automatically instead of having to know about them.
ASSETS = [
    {
        "name": "IBM Plex Sans, IBM Plex Mono",
        "version": "bundled subset (woff2)",
        "license": "OFL-1.1",
        "holder": "Copyright 2017 IBM Corp.",
        "url": "https://github.com/IBM/plex",
    },
    {
        "name": "Space Grotesk",
        "version": "bundled subset (woff2)",
        "license": "OFL-1.1",
        "holder": "Copyright 2020 Florian Karsten",
        "url": "https://github.com/floriankarsten/space-grotesk",
    },
    {
        "name": "htmx",
        "version": "bundled (minified)",
        "license": "0BSD",
        "holder": "Copyright Big Sky Software",
        "url": "https://github.com/bigskysoftware/htmx",
    },
]

# Licence texts our bundled assets need, which no crate source provides.
TEXTS_DIR = ROOT / "scripts" / "license-texts"


def crate_license_texts(pkg_manifest: str) -> list[str]:
    """The licence files a crate actually ships, read from its own source.

    MIT and ISC ask for *that crate's* copyright line, not a generic template,
    so the real file is quoted. Crates that ship none are listed by SPDX
    identifier instead; an incomplete notice should look incomplete rather than
    quietly substitute someone else's copyright.
    """
    src = pathlib.Path(pkg_manifest).parent
    found = []
    for path in sorted(src.iterdir()):
        if not path.is_file():
            continue
        name = path.name.upper()
        if name.startswith(("LICENSE", "LICENCE", "COPYING", "NOTICE")):
            if path.suffix.lower() in {".spdx", ".toml", ".lock"}:
                continue
            try:
                text = path.read_text(encoding="utf-8", errors="replace").strip()
            except OSError:
                continue
            if text:
                found.append(text)
    return found


def workspace_members(meta: dict) -> set[str]:
    return set(meta.get("workspace_members", []))


def crates(meta: dict) -> list[dict]:
    """Third-party packages the workspace links, excluding our own crates."""
    ours = workspace_members(meta)
    out = []
    for pkg in meta["packages"]:
        if pkg["id"] in ours:
            continue
        out.append(
            {
                "name": pkg["name"],
                "version": pkg["version"],
                "license": pkg.get("license") or "(unstated)",
                "url": pkg.get("repository") or "",
                "manifest": pkg.get("manifest_path", ""),
            }
        )
    return sorted(out, key=lambda p: (p["name"].lower(), p["version"]))


def licence_text(spdx: str) -> str | None:
    path = TEXTS_DIR / f"{spdx}.txt"
    return path.read_text() if path.exists() else None


def main() -> int:
    raw = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--all-features"],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    if raw.returncode != 0:
        print(raw.stderr, file=sys.stderr)
        return 1
    meta = json.loads(raw.stdout)
    deps = crates(meta)

    lines: list[str] = []
    lines.append("Third-party notices for Laterite")
    lines.append("=" * 32)
    lines.append("")
    lines.append(
        "Laterite's own code is dual-licensed MIT or Apache-2.0; see LICENSE-MIT\n"
        "and LICENSE-APACHE. This file covers the third-party work a Laterite\n"
        "binary carries: assets compiled into it, and the Rust crates it links.\n"
        "\n"
        "Generated by scripts/gen-notice.py. Do not edit by hand."
    )
    lines.append("")

    lines.append("Bundled assets")
    lines.append("-" * 14)
    lines.append("")
    for a in ASSETS:
        lines.append(f"{a['name']} ({a['version']})")
        lines.append(f"    {a['license']} - {a['holder']}")
        if a["url"]:
            lines.append(f"    {a['url']}")
        lines.append("")

    lines.append(f"Rust crates ({len(deps)})")
    lines.append("-" * 20)
    lines.append("")
    for d in deps:
        suffix = f"  {d['url']}" if d["url"] else ""
        lines.append(f"{d['name']} {d['version']}  [{d['license']}]{suffix}")
    lines.append("")

    # Real licence texts, read from each crate's own source and deduplicated:
    # most MIT files differ only in the copyright line, so identical texts are
    # quoted once with every crate that ships them listed against it.
    by_text: dict[str, list[str]] = {}
    without: list[str] = []
    for d in deps:
        texts = crate_license_texts(d["manifest"]) if d["manifest"] else []
        if not texts:
            without.append(f"{d['name']} {d['version']}  [{d['license']}]")
            continue
        for text in texts:
            by_text.setdefault(text, []).append(f"{d['name']} {d['version']}")

    # Our own crates' NOTICE files, which carry the licences for the assets we
    # bundle. The dependency scan above skips workspace members by design, so
    # these are gathered separately; a consumer's scanner sees them as ordinary
    # crate licence files and needs no special case.
    for crate_dir in sorted((ROOT / "crates").iterdir()):
        own = crate_dir / "NOTICE"
        if own.is_file():
            by_text.setdefault(own.read_text().strip(), []).append(
                f"{crate_dir.name} (bundled assets)"
            )

    lines.append(f"Licence texts ({len(by_text)} distinct)")
    lines.append("-" * 30)
    lines.append("")
    for text, users in sorted(by_text.items(), key=lambda kv: (-len(kv[1]), kv[1][0])):
        lines.append(f"Applies to: {', '.join(sorted(set(users)))}")
        lines.append("")
        lines.append(text)
        lines.append("")
        lines.append("-" * 70)
        lines.append("")

    if without:
        lines.append("Works shipping no licence file")
        lines.append("-" * 30)
        lines.append("")
        lines.append(
            "These declare a licence in their manifest but ship no copy of it.\n"
            "Read the terms at https://spdx.org/licenses/ under the identifier shown."
        )
        lines.append("")
        for row in without:
            lines.append(f"    {row}")
        lines.append("")

    OUT.write_text("\n".join(lines).rstrip() + "\n")
    print(
        f"{OUT.relative_to(ROOT)}: {len(ASSETS)} assets, {len(deps)} crates, "
        f"{len(by_text)} distinct licence texts quoted, "
        f"{len(without)} works shipping none"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
