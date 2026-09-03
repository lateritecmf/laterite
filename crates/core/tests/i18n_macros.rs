//! The `t!`/`tn!`/`tp!` macros build `Text` values that a `Translator` localizes.
//! Compile-time checks (literal-only source, placeholder-argument matching) can't
//! be asserted at runtime; these cover the runtime shape they produce.

use laterite_core::{t, tn, tp, Translator};

#[test]
fn t_builds_and_interpolates() {
    let tr = Translator::new("en");
    assert_eq!(tr.t(&t!("Save")), "Save");
    assert_eq!(tr.t(&t!("Welcome, {name}", name = "Asha")), "Welcome, Asha");
}

#[test]
fn tp_carries_context() {
    // No catalog here, so it falls back to the source; the context only shapes the
    // lookup key.
    let tr = Translator::new("en");
    assert_eq!(tr.t(&tp!("verb", "Open")), "Open");
}

#[test]
fn tn_selects_the_plural_form() {
    let tr = Translator::new("en");
    assert_eq!(tr.t(&tn!("{n} item", "{n} items", n = 1)), "1 item");
    assert_eq!(tr.t(&tn!("{n} item", "{n} items", n = 5)), "5 items");
    // A non-count argument interpolates alongside the count.
    assert_eq!(
        tr.t(&tn!(
            "{name}: {n} file",
            "{name}: {n} files",
            n = 2,
            name = "Docs"
        )),
        "Docs: 2 files"
    );
}
