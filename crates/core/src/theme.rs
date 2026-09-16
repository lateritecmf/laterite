//! Colour mode: light, dark, or follow the system.
//!
//! The admin panel needs this, and so does any public site an application
//! builds, so the mechanism lives here rather than inside either surface. The
//! admin is one consumer of it, not its owner.
//!
//! What is offered is the **mechanism and its contract**, never an appearance:
//! no icons, no toggle markup, no colour tokens. A site brings its own control
//! and its own palette and keys them off the contract below.
//!
//! # The contract
//!
//! ```text
//! <html data-theme="light">   the resolved mode; write CSS against this
//! localStorage["lat-mode"]    the reader's choice: light | dark | auto
//! window.latMode()            read the current choice
//! window.latSetMode(mode)     persist a choice and apply it
//! window.latApplyMode(mode)   apply without persisting
//! ```
//!
//! `auto` follows the operating system *and keeps following it*: a page left
//! open when the system switches at dusk changes with it rather than waiting for
//! a reload. An explicit `light` or `dark` is the reader overriding the system,
//! so it is left alone.
//!
//! # Using it from a page
//!
//! Put [`boot_markup`] in `<head>` before any stylesheet. It must be inline and
//! blocking: a deferred script runs after first paint, and a dark-mode reader
//! would see the page flash white first.
//!
//! ```rust
//! # use laterite_core::theme;
//! let head = format!("<head>{}<link rel=\"stylesheet\" href=\"/site.css\"></head>",
//!                    theme::boot_markup());
//! assert!(head.contains("latApplyMode"));
//! ```
//!
//! Then style against the attribute, and wire any control to `latSetMode`:
//!
//! ```css
//! :root            { --bg: white; --fg: black }
//! [data-theme=dark]{ --bg: black; --fg: white }
//! ```
//!
//! ```js
//! document.querySelector('#to-dark').onclick = () => latSetMode('dark');
//! ```
//!
//! # Content Security Policy
//!
//! An inline script needs `script-src 'unsafe-inline'`, or a nonce or hash. Use
//! [`BOOT_JS`] to serve the same source as a file instead, accepting that a
//! separate request may paint before it arrives.

/// The boot script's JavaScript, without a surrounding `<script>` element.
///
/// Serve it as a file when a Content Security Policy forbids inline script, or
/// embed it yourself with a nonce. Otherwise prefer [`boot_markup`].
pub const BOOT_JS: &str = include_str!("../assets/theme-boot.js");

/// The `localStorage` key holding the reader's choice, for a surface that wants
/// to read or clear it without going through the script.
pub const MODE_KEY: &str = "lat-mode";

/// The three modes, as the strings stored under [`MODE_KEY`].
pub const MODES: [&str; 3] = ["light", "dark", "auto"];

/// The boot script wrapped in a `<script>` element, ready to inline in `<head>`
/// before any stylesheet.
///
/// Returns markup, so a template renders it raw (Askama `|safe`, and the
/// equivalent elsewhere). The content is a compile-time constant with no caller
/// input in it, so there is nothing here to escape.
pub fn boot_markup() -> String {
    format!("<script>{BOOT_JS}</script>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_contract_the_docs_promise_is_the_one_the_script_establishes() {
        // These names are the public surface: a site writes CSS and controls
        // against them, so renaming one silently is a breaking change.
        for name in [
            "latApplyMode",
            "latSetMode",
            "latMode",
            "data-theme",
            MODE_KEY,
        ] {
            assert!(
                BOOT_JS.contains(name),
                "the boot script must define `{name}`"
            );
        }
    }

    #[test]
    fn auto_keeps_following_the_system() {
        // The bug this exists to prevent: resolving once at load and freezing,
        // so a page open through dusk never changes.
        assert!(BOOT_JS.contains("prefers-color-scheme"));
        assert!(
            BOOT_JS.contains("addEventListener") && BOOT_JS.contains("addListener"),
            "must listen for the system changing, with the pre-Safari-14 fallback"
        );
    }

    #[test]
    fn the_markup_is_inlineable() {
        let markup = boot_markup();
        assert!(markup.starts_with("<script>") && markup.ends_with("</script>"));
        // No closing tag inside the source would truncate the element early.
        assert!(!BOOT_JS.contains("</script"));
    }
}
