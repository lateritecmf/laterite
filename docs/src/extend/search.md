# Search and Matching

A lookup that matches text attaches a `SearchProfile`. The profile decides three
things: how text folds, how a query widens into alternatives, and how the result
becomes SQL.

```rust
use laterite_core::search::{Numerals, SearchProfile};

let profile = SearchProfile::folding().with_expander(Numerals);
assert_eq!(profile.fold("  Ávila's  Café "), "avilas cafe");
```

## Folding runs in Rust

Case, diacritic and punctuation folding happen here, never in a database
collation, trigger, or generated column. The three databases Laterite targets
cannot be made to fold identically, and SQLite ships no ICU collation at all. If
the write side and the query side fold differently, matching fails silently and
nothing reports it. Keeping the fold in one place is what makes a profile behave
the same on Postgres, MySQL and SQLite.

The built-in folds are `Lowercase`, `Diacritics` (NFKD, then drop combining
marks), `Punctuation`, and `Whitespace`. `NormalizerChain::standard()` applies all
four in that order.

## The fold applies to the query

A profile folds the text you search *with*. It does not reach values already
stored, so a folded query for `avila` does not find a stored `Ávila` unless the
column it matches against holds folded values too.

This is why `TableSource` folds case only by default. Widen it where the column
is folded to match:

```rust
# use laterite_core::search::{NormalizerChain, SearchProfile};
let profile = SearchProfile::new().with_chain(NormalizerChain::standard());
```

`NormalizerChain::fingerprint()` names the chain (`case+diacritics+punctuation+whitespace`).
Store it beside folded data: when the chain changes, the stored values were folded
by the old one and need rebuilding, and comparing fingerprints is how that gets
noticed.

## Expanders widen a query

An expander turns a folded query into alternatives, query-side only, so adding one
never needs stored data rebuilt. `Numerals` swaps English number words for digits
both ways, on whole tokens (`7 rivers` finds `seven rivers`, while `money` is left
alone). `Aliases` carries pairs that reach each other:

```rust
# use laterite_core::search::{Aliases, SearchProfile};
let profile = SearchProfile::folding()
    .with_expander(Aliases::new().pair("bengaluru", "bangalore"));
```

Fold alias values through the same chain before adding them, or a folded query
will never match them.

`candidates(q)` returns the terms to match: the folded query first, then its
expansions, escaped for `LIKE` and capped at `MAX_CANDIDATES`. Each candidate
becomes one OR'd `LIKE`, and a leading-wildcard `LIKE` is a scan, so the cap
bounds the cost.

## Where it is used

A picker source matches through a profile (`TableSource::with_search`), and a
list's search box matches across the columns marked searchable, which defaults to
the text ones. Both use the case-folding default today; naming a richer profile
from a descriptor comes with the profile registry.

## Matching

`condition(caps, column, q)` builds the `sea_query` condition. `LikeMatcher` is
the portable default: case-folded substring matching with an explicit escape
character, which SQLite requires.

A matcher that needs a database extension implements `applies` against the
capability set and stands down when it is absent, falling back to `LIKE` rather
than failing. One profile therefore stays portable across all three databases.

A blank query, or one that folds away to nothing, yields no candidates and
`condition` returns `None`. That means "no terms", and the caller decides what to
do: a picker that opens as a dropdown adds no clause and lists the first rows.
