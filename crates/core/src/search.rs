//! Text folding and matching for lookups.
//!
//! A [`SearchProfile`] composes three small seams: [`Normalizer`]s fold text,
//! [`QueryExpander`]s widen a folded query into alternatives, and a [`Matcher`]
//! turns those candidates into a `sea_query` condition.
//!
//! **Folding runs here, in Rust, applied identically to the query and to any
//! stored value.** NFKD and punctuation folding cannot be reimplemented
//! identically across Postgres, MySQL and SQLite, so doing it in collations,
//! triggers or generated columns lets the write side and the query side drift
//! apart, which breaks matching silently. Everything else in this module follows
//! from that constraint.
//!
//! ```
//! use laterite_core::search::SearchProfile;
//!
//! let profile = SearchProfile::folding();
//! // Case and diacritics fold, so the query reaches the stored spelling.
//! assert_eq!(profile.fold("Ávila"), "avila");
//! ```

use std::sync::Arc;

use sea_query::{Alias, Condition, Expr, LikeExpr};
use unicode_normalization::UnicodeNormalization;

/// The most candidates a single query fans out to. Each candidate becomes one
/// OR'd `LIKE`, and a leading-wildcard `LIKE` is a scan, so the fan-out is
/// capped rather than left to the expanders.
pub const MAX_CANDIDATES: usize = 8;

/// One text fold, applied to both the query and any stored value.
///
/// `key` names the fold in a chain's [`fingerprint`](NormalizerChain::fingerprint),
/// so it must stay stable once a deployment has folded data with it.
pub trait Normalizer: Send + Sync + 'static {
    fn key(&self) -> &'static str;
    fn fold(&self, text: &str) -> String;
}

/// Unicode-aware lowercase.
pub struct Lowercase;
impl Normalizer for Lowercase {
    fn key(&self) -> &'static str {
        "case"
    }
    fn fold(&self, text: &str) -> String {
        text.to_lowercase()
    }
}

/// Strips diacritics: NFKD, then drop the combining marks, so `Ávila` folds to
/// `avila` and `ﬁ` to `fi`.
pub struct Diacritics;
impl Normalizer for Diacritics {
    fn key(&self) -> &'static str {
        "diacritics"
    }
    fn fold(&self, text: &str) -> String {
        text.nfkd()
            .filter(|c| !is_combining_mark(*c))
            .collect::<String>()
    }
}

/// Whether `c` is a Unicode combining mark (general category M*), the class NFKD
/// splits a diacritic into.
fn is_combining_mark(c: char) -> bool {
    matches!(c as u32,
        0x0300..=0x036F      // combining diacritical marks
        | 0x0483..=0x0489
        | 0x0591..=0x05BD
        | 0x0610..=0x061A
        | 0x064B..=0x065F
        | 0x0670
        | 0x06D6..=0x06DC
        | 0x0711
        | 0x0730..=0x074A
        | 0x07A6..=0x07B0
        | 0x0900..=0x0903    // Indic combining marks
        | 0x093A..=0x094F
        | 0x0951..=0x0957
        | 0x0962..=0x0963
        | 0x0981..=0x0983
        | 0x09BC..=0x09CD
        | 0x0A01..=0x0A03
        | 0x0A3C..=0x0A4D
        | 0x0C00..=0x0C04    // Telugu
        | 0x0C3E..=0x0C56
        | 0x0C81..=0x0C83    // Kannada
        | 0x0CBC..=0x0CD6
        | 0x1AB0..=0x1AFF
        | 0x1DC0..=0x1DFF
        | 0x20D0..=0x20F0
        | 0xFE20..=0xFE2F)
}

/// Folds apostrophes, hyphens and other punctuation away, so `watson's` and
/// `watsons` fold alike.
pub struct Punctuation;
impl Normalizer for Punctuation {
    fn key(&self) -> &'static str {
        "punctuation"
    }
    fn fold(&self, text: &str) -> String {
        text.chars()
            .filter(|c| !c.is_ascii_punctuation() && !matches!(c, '\u{2018}'..='\u{201F}'))
            .collect()
    }
}

/// Collapses runs of whitespace to one space and trims the ends.
pub struct Whitespace;
impl Normalizer for Whitespace {
    fn key(&self) -> &'static str {
        "whitespace"
    }
    fn fold(&self, text: &str) -> String {
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}

/// An ordered set of folds, applied left to right.
///
/// [`fingerprint`](Self::fingerprint) joins the keys. A deployment that maintains
/// a folded column stores the fingerprint beside it: when the chain changes, the
/// stored values were folded by the old chain and need a backfill, and comparing
/// fingerprints is how that is noticed rather than silently mismatching.
#[derive(Clone, Default)]
pub struct NormalizerChain {
    folds: Vec<Arc<dyn Normalizer>>,
}

impl NormalizerChain {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, fold: impl Normalizer) -> Self {
        self.folds.push(Arc::new(fold));
        self
    }

    /// Case, diacritics, punctuation, whitespace: the fold a lookup wants unless
    /// it has a reason to differ.
    pub fn standard() -> Self {
        Self::new()
            .with(Lowercase)
            .with(Diacritics)
            .with(Punctuation)
            .with(Whitespace)
    }

    pub fn fold(&self, text: &str) -> String {
        self.folds
            .iter()
            .fold(text.to_string(), |acc, f| f.fold(&acc))
    }

    /// The chain's identity: its keys in order. Store it beside folded data to
    /// detect a chain change that invalidates it.
    pub fn fingerprint(&self) -> String {
        self.folds
            .iter()
            .map(|f| f.key())
            .collect::<Vec<_>>()
            .join("+")
    }

    pub fn is_empty(&self) -> bool {
        self.folds.is_empty()
    }
}

/// Widens a folded query into alternative spellings. Query-side only: an expander
/// never runs against stored values, so adding one needs no backfill.
pub trait QueryExpander: Send + Sync + 'static {
    /// The alternatives for `folded_q`, excluding the query itself.
    fn expand(&self, folded_q: &str) -> Vec<String>;
}

/// Maps whole folded queries to alternatives, both ways. The mechanism is here;
/// the data belongs to whoever owns the vocabulary (a plugin, or a table).
#[derive(Default)]
pub struct Aliases {
    pairs: Vec<(String, String)>,
}

impl Aliases {
    pub fn new() -> Self {
        Self::default()
    }

    /// Both spellings reach each other. Values are folded by the caller's chain
    /// before being added, or a folded query never matches them.
    pub fn pair(mut self, a: &str, b: &str) -> Self {
        self.pairs.push((a.to_string(), b.to_string()));
        self
    }
}

impl QueryExpander for Aliases {
    fn expand(&self, folded_q: &str) -> Vec<String> {
        let mut out = Vec::new();
        for (a, b) in &self.pairs {
            if a == folded_q {
                out.push(b.clone());
            } else if b == folded_q {
                out.push(a.clone());
            }
        }
        out
    }
}

/// Swaps English number words for digits and back, so `7 rivers` and
/// `seven rivers` reach each other.
pub struct Numerals;

const NUMBER_WORDS: [(&str, &str); 20] = [
    ("zero", "0"),
    ("one", "1"),
    ("two", "2"),
    ("three", "3"),
    ("four", "4"),
    ("five", "5"),
    ("six", "6"),
    ("seven", "7"),
    ("eight", "8"),
    ("nine", "9"),
    ("ten", "10"),
    ("eleven", "11"),
    ("twelve", "12"),
    ("thirteen", "13"),
    ("fourteen", "14"),
    ("fifteen", "15"),
    ("sixteen", "16"),
    ("seventeen", "17"),
    ("eighteen", "18"),
    ("nineteen", "19"),
];

impl QueryExpander for Numerals {
    fn expand(&self, folded_q: &str) -> Vec<String> {
        let mut out = Vec::new();
        for (word, digit) in NUMBER_WORDS {
            // Whole tokens only: a substring swap would turn "money" into
            // "m1y" and "10th" into "tenth" inside unrelated words.
            let swapped_to_digit = swap_token(folded_q, word, digit);
            if swapped_to_digit != folded_q {
                out.push(swapped_to_digit);
            }
            let swapped_to_word = swap_token(folded_q, digit, word);
            if swapped_to_word != folded_q {
                out.push(swapped_to_word);
            }
        }
        out
    }
}

/// Replaces whole space-separated tokens equal to `from` with `to`.
fn swap_token(text: &str, from: &str, to: &str) -> String {
    text.split(' ')
        .map(|t| if t == from { to } else { t })
        .collect::<Vec<_>>()
        .join(" ")
}

/// What a matcher is given: the column to match and the candidate terms, already
/// folded, expanded and escaped.
pub struct MatchCx<'a> {
    pub column: &'a str,
    pub candidates: &'a [String],
}

/// Builds the SQL condition for a set of candidates.
///
/// Synchronous by design: `sea_query`'s expression tree is not `Send`, so a
/// matcher runs inside the scoped build block rather than across an await.
pub trait Matcher: Send + Sync + 'static {
    fn key(&self) -> &'static str;
    /// Whether this matcher can run here. A matcher needing a database extension
    /// checks the capability set and stands down when it is absent, so one
    /// profile stays portable across the three databases.
    fn applies(&self, caps: &crate::capabilities::CapabilitySet) -> bool {
        let _ = caps;
        true
    }
    fn condition(&self, cx: &MatchCx<'_>) -> Condition;
}

/// Case-folded substring matching: the portable default, `LIKE '%term%'` OR'd
/// across candidates with an explicit escape (SQLite requires one).
pub struct LikeMatcher;

impl Matcher for LikeMatcher {
    fn key(&self) -> &'static str {
        "like"
    }
    fn condition(&self, cx: &MatchCx<'_>) -> Condition {
        let mut any = Condition::any();
        for term in cx.candidates {
            any = any.add(
                Expr::expr(sea_query::Func::lower(Expr::col(Alias::new(cx.column))))
                    .like(LikeExpr::new(format!("%{term}%")).escape('\\')),
            );
        }
        any
    }
}

/// Escapes the `LIKE` metacharacters so a user's `%` or `_` matches literally.
pub fn like_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// A composed search behaviour: how text folds, how a query widens, and how the
/// result becomes SQL. A lookup attaches one.
pub struct SearchProfile {
    chain: NormalizerChain,
    expanders: Vec<Arc<dyn QueryExpander>>,
    matchers: Vec<Arc<dyn Matcher>>,
}

impl Default for SearchProfile {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchProfile {
    /// Case-folding only, matched with `LIKE`. What a lookup gets without asking.
    pub fn new() -> Self {
        Self {
            chain: NormalizerChain::new().with(Lowercase),
            expanders: Vec::new(),
            matchers: vec![Arc::new(LikeMatcher)],
        }
    }

    /// The standard fold (case, diacritics, punctuation, whitespace).
    ///
    /// The fold applies to the query always. It reaches stored values only where
    /// the lookup matches against a folded column; against a raw column a folded
    /// query still misses a stored `Ávila`, which is what the folded column is
    /// for.
    pub fn folding() -> Self {
        Self::new().with_chain(NormalizerChain::standard())
    }

    pub fn with_chain(mut self, chain: NormalizerChain) -> Self {
        self.chain = chain;
        self
    }

    pub fn with_expander(mut self, expander: impl QueryExpander) -> Self {
        self.expanders.push(Arc::new(expander));
        self
    }

    /// Adds a matcher ahead of the default, for [`Self::matcher`] to pick when
    /// the deployment supports it.
    pub fn with_matcher(mut self, matcher: impl Matcher) -> Self {
        self.matchers.insert(0, Arc::new(matcher));
        self
    }

    pub fn chain(&self) -> &NormalizerChain {
        &self.chain
    }

    pub fn fold(&self, text: &str) -> String {
        self.chain.fold(text)
    }

    /// The terms to match: the folded query plus its expansions, escaped for
    /// `LIKE` and capped at [`MAX_CANDIDATES`]. Empty when the query folds away
    /// to nothing, which a caller reads as "match nothing".
    pub fn candidates(&self, q: &str) -> Vec<String> {
        let folded = self.chain.fold(q);
        if folded.is_empty() {
            return Vec::new();
        }
        let mut terms = vec![folded.clone()];
        for expander in &self.expanders {
            for alt in expander.expand(&folded) {
                if !alt.is_empty() && !terms.contains(&alt) {
                    terms.push(alt);
                }
            }
        }
        terms.truncate(MAX_CANDIDATES);
        terms.iter().map(|t| like_escape(t)).collect()
    }

    /// The first matcher this deployment supports, falling back to `LIKE`. A
    /// matcher that cannot run is skipped, never an error, so one profile works
    /// on all three databases.
    pub fn matcher(&self, caps: &crate::capabilities::CapabilitySet) -> &dyn Matcher {
        for matcher in &self.matchers {
            if matcher.applies(caps) {
                return matcher.as_ref();
            }
        }
        // `new()` always seeds `LikeMatcher`, which applies everywhere.
        self.matchers
            .last()
            .expect("a profile always carries a matcher")
            .as_ref()
    }

    /// The condition matching `q` against `column`.
    ///
    /// `None` means the query carried no terms (it was blank, or folded away to
    /// nothing). What that means is the caller's to decide: a lookup that opens
    /// as a dropdown adds no clause and lists the first rows, which is the usual
    /// reading. It is never "match nothing" here.
    pub fn condition(
        &self,
        caps: &crate::capabilities::CapabilitySet,
        column: &str,
        q: &str,
    ) -> Option<Condition> {
        let candidates = self.candidates(q);
        if candidates.is_empty() {
            return None;
        }
        Some(self.matcher(caps).condition(&MatchCx {
            column,
            candidates: &candidates,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_folds() {
        assert_eq!(Lowercase.fold("ÁVILA Straße"), "ávila straße");
    }

    #[test]
    fn diacritics_fold_to_their_base_letters() {
        assert_eq!(Diacritics.fold("Ávila"), "Avila");
        assert_eq!(Diacritics.fold("café"), "cafe");
        assert_eq!(Diacritics.fold("Ünïcôdé"), "Unicode");
    }

    /// NFKD is compatibility decomposition, so a ligature splits too.
    #[test]
    fn compatibility_forms_decompose() {
        assert_eq!(Diacritics.fold("ﬁle"), "file");
        assert_eq!(Diacritics.fold("½"), "1⁄2");
    }

    /// A script whose combining marks carry meaning still folds without losing
    /// its base characters.
    #[test]
    fn indic_text_keeps_its_base_characters() {
        let kannada = "ಬೆಂಗಳೂರು";
        let folded = Diacritics.fold(kannada);
        assert!(folded.starts_with('ಬ'), "base letters survive: {folded}");
        assert!(folded.chars().count() < kannada.chars().count());
    }

    #[test]
    fn punctuation_folds_away() {
        assert_eq!(Punctuation.fold("watson's"), "watsons");
        assert_eq!(Punctuation.fold("jean-luc"), "jeanluc");
        assert_eq!(Punctuation.fold("o\u{2019}brien"), "obrien");
    }

    #[test]
    fn whitespace_collapses() {
        assert_eq!(Whitespace.fold("  two   words \n"), "two words");
    }

    #[test]
    fn the_standard_chain_applies_every_fold() {
        let chain = NormalizerChain::standard();
        assert_eq!(chain.fold("  Ávila's   Café "), "avilas cafe");
    }

    #[test]
    fn a_fingerprint_names_the_chain_in_order() {
        assert_eq!(
            NormalizerChain::standard().fingerprint(),
            "case+diacritics+punctuation+whitespace"
        );
        // A different order is a different fingerprint: the folds do not commute.
        let reversed = NormalizerChain::new().with(Punctuation).with(Lowercase);
        assert_eq!(reversed.fingerprint(), "punctuation+case");
    }

    #[test]
    fn numerals_swap_whole_tokens_both_ways() {
        let out = Numerals.expand("7 rivers");
        assert!(out.contains(&"seven rivers".to_string()));
        let out = Numerals.expand("seven rivers");
        assert!(out.contains(&"7 rivers".to_string()));
    }

    /// A substring swap would rewrite unrelated words; only whole tokens move.
    #[test]
    fn numerals_leave_words_containing_a_number_word_alone() {
        assert!(Numerals.expand("money").is_empty());
        assert!(Numerals.expand("nineteenth").is_empty());
        assert!(Numerals.expand("someone").is_empty());
    }

    #[test]
    fn aliases_expand_both_directions() {
        let aliases = Aliases::new().pair("bengaluru", "bangalore");
        assert_eq!(aliases.expand("bengaluru"), ["bangalore"]);
        assert_eq!(aliases.expand("bangalore"), ["bengaluru"]);
        assert!(aliases.expand("mysuru").is_empty());
    }

    #[test]
    fn like_metacharacters_are_escaped() {
        assert_eq!(like_escape("100%"), "100\\%");
        assert_eq!(like_escape("a_b"), "a\\_b");
        assert_eq!(like_escape("back\\slash"), "back\\\\slash");
    }

    #[test]
    fn candidates_fold_expand_and_escape() {
        let profile = SearchProfile::folding().with_expander(Numerals);
        let out = profile.candidates("  7 Rívers ");
        assert_eq!(out[0], "7 rivers", "the folded query comes first");
        assert!(out.contains(&"seven rivers".to_string()));
    }

    #[test]
    fn candidates_escape_a_wildcard_in_the_query() {
        let out = SearchProfile::new().candidates("100%");
        assert_eq!(out, ["100\\%"]);
    }

    #[test]
    fn an_empty_query_yields_no_candidates() {
        assert!(SearchProfile::folding().candidates("   ").is_empty());
        assert!(SearchProfile::folding().candidates("").is_empty());
        // Punctuation-only folds away to nothing, which is not a match-everything.
        assert!(SearchProfile::folding().candidates("!!!").is_empty());
    }

    #[test]
    fn candidates_are_capped() {
        // An expander that always returns more alternatives than the cap allows.
        struct Noisy;
        impl QueryExpander for Noisy {
            fn expand(&self, _q: &str) -> Vec<String> {
                (0..50).map(|i| format!("alt{i}")).collect()
            }
        }
        let out = SearchProfile::new().with_expander(Noisy).candidates("q");
        assert_eq!(out.len(), MAX_CANDIDATES);
    }

    #[test]
    fn duplicate_expansions_are_dropped() {
        struct Same;
        impl QueryExpander for Same {
            fn expand(&self, q: &str) -> Vec<String> {
                vec![q.to_string(), "other".to_string()]
            }
        }
        let out = SearchProfile::new().with_expander(Same).candidates("q");
        assert_eq!(out, ["q", "other"]);
    }

    #[test]
    fn the_default_matcher_is_like() {
        let caps = crate::capabilities::CapabilitySet::default();
        assert_eq!(SearchProfile::new().matcher(&caps).key(), "like");
    }

    #[test]
    fn a_matcher_that_does_not_apply_is_skipped() {
        struct NeedsExtension;
        impl Matcher for NeedsExtension {
            fn key(&self) -> &'static str {
                "trigram"
            }
            fn applies(&self, caps: &crate::capabilities::CapabilitySet) -> bool {
                caps.has("pg_trgm")
            }
            fn condition(&self, _cx: &MatchCx<'_>) -> Condition {
                Condition::any()
            }
        }
        let profile = SearchProfile::new().with_matcher(NeedsExtension);
        let bare = crate::capabilities::CapabilitySet::default();
        assert_eq!(
            profile.matcher(&bare).key(),
            "like",
            "falls back, not errors"
        );
    }

    #[test]
    fn no_condition_for_a_query_that_folds_away() {
        let caps = crate::capabilities::CapabilitySet::default();
        assert!(SearchProfile::folding()
            .condition(&caps, "name", "  ")
            .is_none());
    }
}
