//! Dashboard widgets: the blocks the dashboard is made of, contributed by the
//! framework and by modules, each gated by its own permission and placed by the
//! arrangement engine, with every operator free to choose and order their own.
//!
//! A widget is a [`Widget`] registered through
//! [`AdminRegistry::add_widget`](crate::AdminRegistry). It renders a fragment
//! of markup for one operator; the dashboard frames it, and a widget that
//! declares a refresh interval is fetched again on its own, without the page.

use crate::html::Markup;
use laterite_auth::AuthenticatedUser;
use laterite_core::arrange::{Arranged, Hierarchy, Policy, Position};
use laterite_core::{Db, Text};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// How wide a widget sits in the dashboard's six-column grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Size {
    /// A third of the row.
    Third,
    /// Half the row.
    #[default]
    Half,
    /// The whole row.
    Full,
}

impl Size {
    pub(crate) fn class(self) -> &'static str {
        match self {
            Self::Third => "lat-widget--third",
            Self::Half => "lat-widget--half",
            Self::Full => "lat-widget--full",
        }
    }
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Third => "third",
            Self::Half => "half",
            Self::Full => "full",
        }
    }
    pub(crate) fn parse(key: &str) -> Option<Self> {
        match key {
            "third" => Some(Self::Third),
            "half" => Some(Self::Half),
            "full" => Some(Self::Full),
            _ => None,
        }
    }
}

/// What a widget renders from: the request's operator and shell, the
/// database, and the size the operator chose for it.
pub struct WidgetCx<'a> {
    pub(crate) state: &'a crate::AdminState,
    pub shell: &'a crate::Shell,
    pub user: &'a AuthenticatedUser,
    pub size: Size,
}

impl WidgetCx<'_> {
    pub fn db(&self) -> &Db {
        &self.state.db
    }
    /// The admin's mount path, for a link into the panel.
    pub fn base(&self) -> &str {
        &self.shell.base
    }
    /// Localizes a string for this operator.
    pub fn t(&self, text: &Text) -> String {
        self.shell.tt(text)
    }
}

/// A block on the dashboard.
#[laterite_core::strata::async_trait]
pub trait Widget: Send + Sync {
    /// `vendor.name`, unique across modules.
    fn code(&self) -> &str;
    /// Shown as the widget's heading and in the Manage dialog.
    fn label(&self) -> Text;
    /// A line under the label in the Manage dialog.
    fn description(&self) -> Option<Text> {
        None
    }
    /// The permission an operator needs to see it; `None` shows it to every
    /// operator.
    fn permission(&self) -> Option<&str> {
        None
    }
    /// The width it takes until an operator chooses another.
    fn size(&self) -> Size {
        Size::Half
    }
    /// Seconds between refreshes of its body, fetched on its own; `None` renders
    /// once with the page.
    fn refresh(&self) -> Option<u32> {
        None
    }
    /// The widget's body. An `Err` renders as a notice inside the frame, never
    /// as a failed page.
    async fn render(&self, cx: &WidgetCx<'_>) -> Result<Markup, String>;
}

/// A registered widget with its place among the others.
#[derive(Clone)]
pub struct WidgetReg {
    pub widget: Arc<dyn Widget>,
    pub position: Position,
    label: String,
}

impl WidgetReg {
    pub fn new(widget: Arc<dyn Widget>) -> Self {
        let label = widget.label().source().to_string();
        Self {
            widget,
            position: Position::new().slot(laterite_core::arrange::slot::APPLICATION),
            label,
        }
    }
    /// Where it sits by default. See [`laterite_core::arrange`].
    pub fn position(mut self, position: Position) -> Self {
        self.position = position;
        self
    }
    pub fn code(&self) -> &str {
        self.widget.code()
    }
}

impl Arranged for WidgetReg {
    fn key(&self) -> &str {
        self.widget.code()
    }
    fn position(&self) -> &Position {
        &self.position
    }
    fn label(&self) -> &str {
        &self.label
    }
}

/// The dashboard's hierarchy: deployments and operators may rearrange it.
pub const HIERARCHY: &str = "backend.dashboard";

pub(crate) fn hierarchy() -> Hierarchy {
    Hierarchy::new(HIERARCHY)
        .policy(Policy::Adjustable)
        .operator_adjustable()
}

/// The preference holding an operator's own dashboard.
pub(crate) const PREFERENCE: &str = "dashboard.layout";

/// An operator's dashboard: which widgets, in what order, at what size.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Layout {
    pub widgets: Vec<Placed>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Placed {
    pub code: String,
    #[serde(default)]
    pub size: Size,
}

impl Layout {
    pub(crate) fn parse(json: &str) -> Option<Self> {
        serde_json::from_str(json).ok()
    }
}

/// The widgets an operator may see, in the arranged order.
pub(crate) fn available<'a>(
    regs: &'a [WidgetReg],
    deployment: Option<&[String]>,
    allows: &dyn Fn(&str) -> bool,
) -> Vec<&'a WidgetReg> {
    let mut items: Vec<&WidgetReg> = regs
        .iter()
        .filter(|r| r.widget.permission().is_none_or(allows))
        .collect();
    // Arrange by index, since the engine wants owned items.
    let keyed: Vec<Keyed<'_>> = items
        .iter()
        .enumerate()
        .map(|(i, r)| Keyed { at: i, reg: r })
        .collect();
    let overrides = laterite_core::arrange::Overrides {
        deployment,
        operator: None,
    };
    if let Ok(arranged) = laterite_core::arrange::arrange(&hierarchy(), keyed, overrides) {
        let order: Vec<usize> = arranged.into_iter().map(|k| k.at).collect();
        items = order.into_iter().map(|i| items[i]).collect();
    }
    items
}

struct Keyed<'a> {
    at: usize,
    reg: &'a WidgetReg,
}

impl Arranged for Keyed<'_> {
    fn key(&self) -> &str {
        self.reg.key()
    }
    fn position(&self) -> &Position {
        self.reg.position()
    }
    fn label(&self) -> &str {
        self.reg.label()
    }
}

/// The widgets the dashboard shows: the operator's layout where one is saved,
/// restricted to what they may see, else every available widget at its own
/// size.
pub(crate) fn resolve<'a>(
    layout: Option<&Layout>,
    available: &[&'a WidgetReg],
) -> Vec<(&'a WidgetReg, Size)> {
    match layout {
        Some(layout) => layout
            .widgets
            .iter()
            .filter_map(|p| {
                available
                    .iter()
                    .find(|r| r.code() == p.code)
                    .map(|r| (*r, p.size))
            })
            .collect(),
        None => available.iter().map(|r| (*r, r.widget.size())).collect(),
    }
}

/// The frame around a widget's body: the heading, the size class, the refresh
/// attributes, and the body or the notice a failed render leaves.
pub(crate) fn frame(
    reg: &WidgetReg,
    size: Size,
    base: &str,
    label: &str,
    body: Result<Markup, String>,
    failed: &str,
) -> String {
    let code = crate::form::escape_attr(reg.code());
    let refresh = reg
        .widget
        .refresh()
        .map(|s| {
            format!(
                r#" hx-get="{base}/dashboard/widget/{code}?size={}" hx-trigger="every {s}s" hx-swap="innerHTML""#,
                size.key()
            )
        })
        .unwrap_or_default();
    let inner = match body {
        Ok(markup) => markup.into_string(),
        Err(e) => {
            tracing::warn!(widget = reg.code(), error = %e, "widget failed to render");
            format!(
                r#"<p class="lat-widget__failed">{}</p>"#,
                crate::form::escape_attr(failed)
            )
        }
    };
    format!(
        r#"<section class="lat-widget {}" data-widget="{code}" aria-labelledby="widget-{code}"><h2 class="lat-widget__title" id="widget-{code}">{}</h2><div class="lat-widget__body"{refresh}>{inner}</div></section>"#,
        size.class(),
        crate::form::escape_attr(label),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed(&'static str, Option<&'static str>, Size);

    #[laterite_core::strata::async_trait]
    impl Widget for Fixed {
        fn code(&self) -> &str {
            self.0
        }
        fn label(&self) -> Text {
            Text::dynamic(self.0)
        }
        fn permission(&self) -> Option<&str> {
            self.1
        }
        fn size(&self) -> Size {
            self.2
        }
        async fn render(&self, _cx: &WidgetCx<'_>) -> Result<Markup, String> {
            Markup::from_template(&Body).map_err(|e| e.to_string())
        }
    }

    #[derive(askama::Template)]
    #[template(source = "<p>x</p>", ext = "html")]
    struct Body;

    fn regs() -> Vec<WidgetReg> {
        vec![
            WidgetReg::new(Arc::new(Fixed(
                "acme.sales",
                Some("acme.sales"),
                Size::Full,
            )))
            .position(Position::new().slot(500)),
            WidgetReg::new(Arc::new(Fixed("backend.welcome", None, Size::Half)))
                .position(Position::new().slot(0)),
            WidgetReg::new(Arc::new(Fixed("backend.status", None, Size::Third)))
                .position(Position::new().slot(900)),
        ]
    }

    #[test]
    fn available_filters_by_permission_and_arranges_by_slot() {
        let regs = regs();
        let none = available(&regs, None, &|_| false);
        assert_eq!(
            none.iter().map(|r| r.code()).collect::<Vec<_>>(),
            vec!["backend.welcome", "backend.status"]
        );
        let all = available(&regs, None, &|_| true);
        assert_eq!(
            all.iter().map(|r| r.code()).collect::<Vec<_>>(),
            vec!["backend.welcome", "acme.sales", "backend.status"]
        );
        let moved = ["backend.status".to_string()];
        let deployed = available(&regs, Some(&moved), &|_| true);
        assert_eq!(deployed[0].code(), "backend.status");
    }

    #[test]
    fn a_layout_keeps_its_order_and_sizes_and_drops_what_it_may_not_show() {
        let regs = regs();
        let avail = available(&regs, None, &|_| false);
        let layout = Layout::parse(
            r#"{"widgets":[{"code":"backend.status","size":"full"},{"code":"acme.sales"},{"code":"gone"},{"code":"backend.welcome","size":"third"}]}"#,
        )
        .unwrap();
        let shown = resolve(Some(&layout), &avail);
        assert_eq!(
            shown
                .iter()
                .map(|(r, s)| (r.code(), *s))
                .collect::<Vec<_>>(),
            vec![
                ("backend.status", Size::Full),
                ("backend.welcome", Size::Third)
            ]
        );
        let default = resolve(None, &avail);
        assert_eq!(default[0].1, Size::Half);
        assert_eq!(default[1].1, Size::Third);
    }

    #[test]
    fn a_failed_body_renders_a_notice_inside_the_frame() {
        let regs = regs();
        let html = frame(
            &regs[1],
            Size::Half,
            "/admin",
            "Welcome",
            Err("boom".into()),
            "This widget could not be shown.",
        );
        assert!(html.contains("lat-widget--half"));
        assert!(html.contains("This widget could not be shown."));
        assert!(!html.contains("boom"));
        assert!(!html.contains("hx-get"));
    }
}
