#!/bin/sh
#
# check-bundled-assets.sh: every file compiled into a Laterite binary must be
# accounted for in a crate's NOTICE.
#
# The repository's NOTICE is generated from `cargo metadata`, so a new *crate*
# cannot slip in unlisted: CI regenerates and fails on drift. A new **asset** is
# the hole that check cannot see. `include_bytes!("../assets/fonts/new.woff2")`
# adds a font to every binary and changes no manifest, so nothing notices that
# its licence was never recorded. That is exactly how the IBM Plex and Space
# Grotesk fonts came to be shipped for a month with no copy of the OFL anywhere.
#
# This walks every `include_bytes!` / `include_str!` path in the workspace and
# sorts it into one of two buckets:
#
#   ours        written for Laterite; no third-party notice is owed
#   attributed  third-party, and named in the crate's NOTICE
#
# Anything in neither bucket fails. Adding a third-party asset therefore forces a
# deliberate choice: record it, or declare it ours.
#
# Runs in CI beside the NOTICE drift check, which is the other half of the same
# guarantee: that one catches an unlisted crate, this one an unlisted asset.
#
# Usage: sh scripts/check-bundled-assets.sh   (from the repository root)

cd "$(dirname "$0")/.." || exit 2

# Assets written for Laterite. A pattern here is a claim that we wrote the file
# and owe nobody a notice, so keep it narrow: name files, not directories.
ours='^(laterite\.(css|js)|theme-boot\.js|mark\.(svg|png)|NOTICE\.txt|fields/ref-picker\.(css|js))$'

# Third-party families, each of which must appear in a crate NOTICE. The left
# side matches the path; the right side is the string that must be present.
attributed_patterns='fonts/ibm-plex:IBM Plex
fonts/space-grotesk:Space Grotesk
vendor/htmx:htmx
icons/:Lucide'

fail=0
unaccounted=''

paths=$(grep -rhoE 'include_(bytes|str)!\("[^"]+"\)' --include='*.rs' crates/ 2>/dev/null \
        | sed -E 's/include_(bytes|str)!\("//; s/"\)//' \
        | sed -E 's#^\.\./assets/##' \
        | sort -u)

for path in $paths; do
    # Ours: nothing owed.
    if echo "$path" | grep -qE "$ours"; then
        continue
    fi

    # Third-party: the crate NOTICE must actually name it.
    matched=0
    for rule in $attributed_patterns; do
        pattern=$(echo "$rule" | cut -d: -f1)
        needle=$(echo "$rule" | cut -d: -f2-)
        case "$path" in
            *"$pattern"*)
                matched=1
                if ! grep -rqF "$needle" crates/*/NOTICE 2>/dev/null; then
                    echo "unattributed: $path" >&2
                    echo "    matches the '$pattern' family, but no crate NOTICE names '$needle'" >&2
                    fail=1
                fi
                ;;
        esac
    done

    if [ "$matched" -eq 0 ]; then
        unaccounted="$unaccounted  $path
"
        fail=1
    fi
done

if [ -n "$unaccounted" ]; then
    echo "These files are compiled into the binary and accounted for nowhere:" >&2
    printf '%s' "$unaccounted" >&2
    echo "" >&2
    echo "Either add the file to the 'ours' pattern in this script (if we wrote it)," >&2
    echo "or record its licence in the bundling crate's NOTICE and add a rule here." >&2
fi

if [ "$fail" -ne 0 ]; then
    exit 1
fi

count=$(echo "$paths" | grep -c .)
echo "Bundled assets accounted for: $count files, all ours or attributed."
