# Search

A `SearchProfile` sets how a lookup folds a query and widens it before
matching. A list's search box folds case over its searchable columns and
takes no profile.

## Attach a profile to a picker source

```rust
use laterite_admin::picker::TableSource;
use laterite_core::search::{Numerals, SearchProfile};

let source = TableSource::new("acme_cities", "id", "name_folded")
    .with_search(SearchProfile::folding().with_expander(Numerals));

let profile = SearchProfile::folding();
assert_eq!(profile.fold("  Ávila's  Café "), "avilas cafe");
```

Constructor or method | Description
--- | ---
`SearchProfile::new()` | Folds case only, matched by substring. The default of every lookup.
`SearchProfile::folding()` | Folds case, diacritics, punctuation and whitespace.
`with_chain(chain)` | A `NormalizerChain` of your own.
`with_expander(expander)` | Adds a way of widening the query; may repeat.
`with_matcher(matcher)` | A matcher of your own, used when the database supports it.
`fold(text)` | The folded form of `text`.
`candidates(query)` | The folded query and its expansions.

Normalizer | Effect
--- | ---
`Lowercase` | Case.
`Diacritics` | Accents removed.
`Punctuation` | Removed.
`Whitespace` | Collapsed.

`NormalizerChain::standard()` applies all four in order;
`NormalizerChain::new().with(..)` builds one by hand. A profile folds the
query, not stored values: widen it only where the column holds folded values.
`NormalizerChain::fingerprint()` names the chain,
`case+diacritics+punctuation+whitespace`; keep it beside folded data and
compare after a chain change.

## Turn on a list's search box

```yaml
list:
  search: { prompt: Search by title, on_enter: true }
  columns:
    title: { label: Title, searchable: true }
    body:  { label: Body, searchable: false }
```

Option | Description
--- | ---
`search.enabled` | `false` removes the box. On by default.
`search.prompt` | The placeholder. "Search" when unset.
`search.on_enter` | Ask on Enter only. Off by default: the list narrows as the operator types.
`<column>.searchable` | Whether the box matches this column. Text columns are searchable by default; other types are not.

## Add an alias

```rust
use laterite_core::search::{Aliases, SearchProfile};

let profile = SearchProfile::folding()
    .with_expander(Aliases::new().pair("bengaluru", "bangalore"));
```

`Aliases::pair(a, b)` makes each find the other. Fold alias values through
the same chain before adding them.

## Widen matching

```rust
use laterite_core::search::{NormalizerChain, Numerals, SearchProfile};

let profile = SearchProfile::new()
    .with_chain(NormalizerChain::standard())
    .with_expander(Numerals);
```

Expander | Effect
--- | ---
`Numerals` | English number words and digits, both ways, on whole tokens: `7 rivers` finds `seven rivers`.
`Aliases` | Pairs that reach each other.

An expander widens the query alone; stored data is untouched.
