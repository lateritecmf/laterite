//! Typed admin chrome for a screen that composes a page by hand: the same
//! buttons, toolbars and badges the framework's own screens render, built as
//! values and rendered through the shell, so a module never writes the class
//! names itself.

use crate::form::escape_attr;
use crate::html::Markup;
use crate::Shell;
use laterite_core::Text;

/// How a button reads: the one action on a page, a secondary one, a
/// destructive one, or a quiet one in a row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Style {
    Primary,
    #[default]
    Secondary,
    Danger,
    Ghost,
}

impl Style {
    fn class(self) -> &'static str {
        match self {
            Style::Primary => "lat-btn--primary",
            Style::Secondary => "lat-btn--secondary",
            Style::Danger => "lat-btn--danger",
            Style::Ghost => "lat-btn--ghost",
        }
    }
}

/// What a button does: opens a page, or posts to a path with the request
/// token, asking first when a confirmation is set.
#[derive(Clone, Debug)]
enum Action {
    Link(String),
    Post(String),
}

/// A button: a link styled as one, or a one-field form that posts.
#[derive(Clone, Debug)]
pub struct Button {
    label: Text,
    action: Action,
    style: Style,
    small: bool,
    icon: Option<String>,
    confirm: Option<Text>,
}

impl Button {
    /// A button that opens `href`.
    pub fn link(label: impl Into<Text>, href: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            action: Action::Link(href.into()),
            style: Style::Secondary,
            small: false,
            icon: None,
            confirm: None,
        }
    }

    /// A button that posts to `action` with the request token.
    pub fn post(label: impl Into<Text>, action: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            action: Action::Post(action.into()),
            style: Style::Secondary,
            small: false,
            icon: None,
            confirm: None,
        }
    }

    pub fn style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }

    pub fn primary(self) -> Self {
        self.style(Style::Primary)
    }

    pub fn danger(self) -> Self {
        self.style(Style::Danger)
    }

    pub fn ghost(self) -> Self {
        self.style(Style::Ghost)
    }

    /// The row-sized button.
    pub fn small(mut self) -> Self {
        self.small = true;
        self
    }

    /// An icon from the icon set before the label.
    pub fn icon(mut self, name: impl Into<String>) -> Self {
        self.icon = Some(name.into());
        self
    }

    /// Asks this question before a post goes.
    pub fn confirm(mut self, question: impl Into<Text>) -> Self {
        self.confirm = Some(question.into());
        self
    }

    pub fn render(&self, shell: &Shell) -> Markup {
        let mut class = format!("lat-btn {}", self.style.class());
        if self.small {
            class.push_str(" lat-btn--sm");
        }
        let icon = self
            .icon
            .as_deref()
            .map(|name| format!(r#"<span class="lat-btn__icon">{}</span>"#, shell.icon(name)))
            .unwrap_or_default();
        let label = escape_attr(&shell.tt(&self.label));
        let html = match &self.action {
            Action::Link(href) => format!(
                r#"<a class="{class}" href="{}">{icon}{label}</a>"#,
                escape_attr(href)
            ),
            Action::Post(action) => {
                let confirm = self
                    .confirm
                    .as_ref()
                    .map(|q| format!(r#" data-lat-confirm="{}""#, escape_attr(&shell.tt(q))))
                    .unwrap_or_default();
                format!(
                    r#"<form method="post" action="{}"><input type="hidden" name="_csrf" value="{}"><button class="{class}" type="submit"{confirm}>{icon}{label}</button></form>"#,
                    escape_attr(action),
                    escape_attr(&shell.csrf_token)
                )
            }
        };
        Markup::from_override(html)
    }
}

/// A row of buttons under a heading, as a page's or a section's toolbar.
#[derive(Clone, Debug, Default)]
pub struct Toolbar {
    title: Option<Text>,
    buttons: Vec<Button>,
    section: bool,
}

impl Toolbar {
    /// A page toolbar headed by `title`.
    pub fn new(title: impl Into<Text>) -> Self {
        Self {
            title: Some(title.into()),
            buttons: Vec::new(),
            section: false,
        }
    }

    /// A section's toolbar, smaller, headed by `title`.
    pub fn section(title: impl Into<Text>) -> Self {
        Self {
            title: Some(title.into()),
            buttons: Vec::new(),
            section: true,
        }
    }

    pub fn button(mut self, button: Button) -> Self {
        self.buttons.push(button);
        self
    }

    pub fn render(&self, shell: &Shell) -> Markup {
        let class = if self.section {
            "lat-toolbar lat-toolbar--section"
        } else {
            "lat-toolbar"
        };
        let title = match &self.title {
            Some(title) if self.section => format!("<h2>{}</h2>", escape_attr(&shell.tt(title))),
            Some(title) => format!("<h1>{}</h1>", escape_attr(&shell.tt(title))),
            None => String::new(),
        };
        let actions = if self.buttons.is_empty() {
            String::new()
        } else {
            let inner: String = self
                .buttons
                .iter()
                .map(|b| b.render(shell).into_string())
                .collect();
            format!(r#"<div class="lat-toolbar__actions">{inner}</div>"#)
        };
        Markup::from_override(format!(r#"<div class="{class}">{title}{actions}</div>"#))
    }
}

/// A small label beside a value: a state, a count, a note.
#[derive(Clone, Debug)]
pub struct Badge {
    label: Text,
}

impl Badge {
    pub fn new(label: impl Into<Text>) -> Self {
        Self {
            label: label.into(),
        }
    }

    pub fn render(&self, shell: &Shell) -> Markup {
        Markup::from_override(format!(
            r#"<span class="lat-badge">{}</span>"#,
            escape_attr(&shell.tt(&self.label))
        ))
    }
}

/// A card: the panel's framed block, around whatever it is given.
#[derive(Clone, Debug)]
pub struct Card {
    body: Markup,
}

impl Card {
    pub fn new(body: Markup) -> Self {
        Self { body }
    }

    pub fn render(&self) -> Markup {
        Markup::from_override(format!(
            r#"<div class="lat-card">{}</div>"#,
            self.body.clone().into_string()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_button_carries_its_style_size_and_icon() {
        let shell = Shell::test();
        let html = Button::link("New post", "/admin/posts/new")
            .primary()
            .small()
            .icon("plus")
            .render(&shell)
            .into_string();
        assert!(
            html.starts_with(
                r#"<a class="lat-btn lat-btn--primary lat-btn--sm" href="/admin/posts/new">"#
            ),
            "{html}"
        );
        assert!(
            html.contains("lat-btn__icon") && html.ends_with("New post</a>"),
            "{html}"
        );
    }

    #[test]
    fn a_post_button_is_a_form_with_the_token_and_asks_first() {
        let shell = Shell::test();
        let html = Button::post("Rebuild", "/admin/reports/rebuild")
            .danger()
            .confirm("Rebuild every report?")
            .render(&shell)
            .into_string();
        assert!(
            html.starts_with(r#"<form method="post" action="/admin/reports/rebuild">"#),
            "{html}"
        );
        assert!(
            html.contains(r#"name="_csrf""#)
                && html.contains(r#"data-lat-confirm="Rebuild every report?""#),
            "{html}"
        );
        assert!(
            html.contains("lat-btn--danger") && !html.contains("lat-btn--sm"),
            "{html}"
        );
    }

    #[test]
    fn a_section_toolbar_heads_its_buttons_and_text_is_escaped() {
        let shell = Shell::test();
        let html = Toolbar::section("Notes <b>")
            .button(Button::link("Add", "/x"))
            .render(&shell)
            .into_string();
        assert!(html.starts_with(r#"<div class="lat-toolbar lat-toolbar--section"><h2>Notes &lt;b&gt;</h2><div class="lat-toolbar__actions">"#), "{html}");
        assert_eq!(
            Badge::new("Stale").render(&shell).into_string(),
            r#"<span class="lat-badge">Stale</span>"#
        );
        assert_eq!(
            Card::new(Markup::from_override("<p>x</p>".to_string()))
                .render()
                .into_string(),
            r#"<div class="lat-card"><p>x</p></div>"#
        );
    }
}
