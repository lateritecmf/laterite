# Search and Matching

A text lookup attaches a `SearchProfile`: how text folds, how a query widens,
and how it becomes SQL.

```rust
use laterite_core::search::{Numerals, SearchProfile};

let profile = SearchProfile::folding().with_expander(Numerals);
assert_eq!(profile.fold("  Ávila's  Café "), "avilas cafe");
```

## Fold text

Folding runs in Rust, never in a database collation.

Normalizer | Effect
--- | ---
`Lowercase` | Case.
`Diacritics` | NFKD, then combining marks dropped.
`Punctuation` | Removed.
`Whitespace` | Collapsed.

`NormalizerChain::standard()` applies all four in order. `SearchProfile::folding()`
folds case only. A profile folds the query, not stored values, so widen it only
where the column holds folded values:

```rust
use laterite_core::search::{NormalizerChain, SearchProfile};

let profile = SearchProfile::new().with_chain(NormalizerChain::standard());
```

`NormalizerChain::fingerprint()` names the chain,
`case+diacritics+punctuation+whitespace`. Store it beside folded data and
compare after a chain change.

## Widen a query

An expander turns the folded query into alternatives. Query-side only; stored
data is untouched.

Expander | Effect
--- | ---
`Numerals` | English number words and digits, both ways, on whole tokens: `7 rivers` finds `seven rivers`.
`Aliases` | Pairs that reach each other.

```rust
use laterite_core::search::{Aliases, SearchProfile};

let profile = SearchProfile::folding()
    .with_expander(Aliases::new().pair("bengaluru", "bangalore"));
```

Fold alias values through the same chain before adding them. `candidates(q)`
returns the folded query and its expansions, escaped for `LIKE`, capped at
`MAX_CANDIDATES`.

## Match

`condition(caps, column, q)` builds the `sea_query` condition; `None` for a
blank query, and the caller decides. `LikeMatcher` is the default: case-folded
substring matching with an explicit escape character. A matcher needing a
database extension implements `applies` against the capability set and stands
down to `LIKE` when it is absent.

## Where it applies

Surface | Profile
--- | ---
Picker source | `TableSource::with_search(profile)`.
List search box | The columns marked searchable, text columns by default. Case-folding.
