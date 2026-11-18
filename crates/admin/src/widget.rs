//! Dashboard widgets: the blocks the dashboard is made of, contributed by the
//! framework and by modules, each gated by its own permission and placed by the
//! arrangement engine, with every operator free to choose and order their own.
//!
//! A widget is a [`Widget`] registered through
//! [`AdminRegistry::add_widget`](crate::AdminRegistry). It renders a fragment
//! of markup for one operator; the dashboard frames it, and a widget that
//! declares a refresh interval is fetched again on its own, without the page.

use crate::board::{Board, BoardCx, Kind, Layout, Placement, COLUMNS};
use crate::html::Markup;
use laterite_auth::AuthenticatedUser;
use laterite_core::arrange::{Arranged, Hierarchy, Policy, Position};
use laterite_core::{Db, Text};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

/// How wide a widget sits: a third, half or all of the dashboard's row.
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
    /// The size a width of the board's twenty columns reads as.
    pub(crate) fn of_width(width: u8) -> Self {
        match width {
            0..=8 => Self::Third,
            9..=14 => Self::Half,
            _ => Self::Full,
        }
    }
}

/// What a widget renders from: the request's operator and shell, the
/// database, and the size the operator chose for it.
pub struct WidgetCx<'a> {
    pub(crate) ctx: &'a crate::routes::RouteCtx,
    /// The deployment's display timezone, by name.
    pub(crate) timezone: &'a str,
    /// The request's page shell: translator, icons and admin path.
    pub shell: &'a crate::Shell,
    /// The signed-in user the dashboard is rendered for.
    pub user: &'a AuthenticatedUser,
    /// The width the user chose for the widget.
    pub size: Size,
}

impl WidgetCx<'_> {
    /// Returns the application's database handle.
    pub fn db(&self) -> &Db {
        self.ctx.db()
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
    /// The widget being registered.
    pub widget: Arc<dyn Widget>,
    /// Where the widget sits on the dashboard by default.
    pub position: Position,
    label: String,
}

impl WidgetReg {
    /// Returns a registration for `widget` in the application band.
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
    /// Returns the widget's `vendor.name` code.
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

/// The preference an operator's dashboard was kept under before boards.
pub(crate) const LEGACY_PREFERENCE: &str = "dashboard.layout";
/// The home dashboard's board.
pub(crate) const HOME: &str = "home";

/// A registered widget as a kind the home board places.
struct Home {
    reg: WidgetReg,
    timezone: String,
}

#[laterite_core::strata::async_trait]
impl Kind for Home {
    fn code(&self) -> &str {
        self.reg.code()
    }
    fn label(&self) -> Text {
        self.reg.widget.label()
    }
    fn description(&self) -> Option<Text> {
        self.reg.widget.description()
    }
    fn permission(&self) -> Option<&str> {
        self.reg.widget.permission()
    }
    fn default_width(&self) -> u8 {
        match self.reg.widget.size() {
            Size::Third => 7,
            Size::Half => 10,
            Size::Full => COLUMNS,
        }
    }
    fn min_width(&self) -> u8 {
        5
    }
    fn once(&self) -> bool {
        true
    }
    fn title(&self, shell: &crate::Shell, _properties: &Value) -> Option<String> {
        Some(shell.tt(&self.reg.widget.label()))
    }
    fn refresh(&self, _properties: &Value) -> Option<u32> {
        self.reg.widget.refresh()
    }
    async fn render(&self, cx: &BoardCx<'_>, _properties: &Value) -> Result<Markup, String> {
        let widget = WidgetCx {
            ctx: cx.ctx,
            timezone: &self.timezone,
            shell: cx.shell,
            user: cx.user,
            size: Size::of_width(cx.width),
        };
        self.reg.widget.render(&widget).await.map_err(|e| {
            tracing::warn!(widget = self.reg.code(), error = %e, "widget failed to render");
            cx.shell.t("This widget could not be shown.")
        })
    }
}

/// Widths for a run of widgets, in rows: thirds sit three to a row, halves
/// two, and a row's widths fill its twenty columns where they can.
pub(crate) fn flow(sizes: &[Size]) -> Vec<Vec<u8>> {
    let mut rows: Vec<Vec<Size>> = Vec::new();
    let mut used = 0u8;
    for size in sizes {
        let sixths = match size {
            Size::Third => 2,
            Size::Half => 3,
            Size::Full => 6,
        };
        if rows.is_empty() || used + sixths > 6 {
            rows.push(Vec::new());
            used = 0;
        }
        used += sixths;
        if let Some(row) = rows.last_mut() {
            row.push(*size);
        }
    }
    rows.into_iter()
        .map(|row| {
            let thirds = row.iter().filter(|s| **s == Size::Third).count();
            let mut third = 0;
            row.iter()
                .map(|size| match size {
                    Size::Full => COLUMNS,
                    Size::Half => 10,
                    // Three thirds are 7, 7 and 6; beside a half, a third is 10.
                    Size::Third if thirds == 1 && row.len() == 2 => 10,
                    Size::Third => {
                        third += 1;
                        if third == 3 {
                            6
                        } else {
                            7
                        }
                    }
                })
                .collect()
        })
        .collect()
}

/// The home dashboard: every registered widget as a kind of its own, placed
/// in the deployment's order at its own size, each person free to arrange
/// theirs.
pub(crate) fn home_board(
    regs: &[WidgetReg],
    deployment: Option<&[String]>,
    timezone: &str,
) -> Board {
    let arranged = available(regs, deployment, &|_| true);
    let sizes: Vec<Size> = arranged.iter().map(|r| r.widget.size()).collect();
    let mut widgets = arranged.iter();
    let rows = flow(&sizes)
        .into_iter()
        .map(|row| {
            row.into_iter()
                .filter_map(|width| {
                    widgets
                        .next()
                        .map(|r| Placement::new(r.code(), r.code(), width))
                })
                .collect()
        })
        .collect();
    let mut board = Board::new(HOME, Layout::rows(rows)).editing(crate::board::Editing::Personal);
    for reg in regs {
        board = board.with_kind(Arc::new(Home {
            reg: reg.clone(),
            timezone: timezone.to_string(),
        }));
    }
    board
}

/// An operator's dashboard as it was kept before boards: which widgets, in
/// what order, at what size.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct LegacyLayout {
    pub widgets: Vec<LegacyPlaced>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct LegacyPlaced {
    pub code: String,
    #[serde(default)]
    pub size: Size,
}

impl LegacyLayout {
    /// The same choice as a board layout.
    pub(crate) fn converted(&self) -> Layout {
        let sizes: Vec<Size> = self.widgets.iter().map(|w| w.size).collect();
        let mut widgets = self.widgets.iter();
        Layout::rows(
            flow(&sizes)
                .into_iter()
                .map(|row| {
                    row.into_iter()
                        .filter_map(|width| {
                            widgets
                                .next()
                                .map(|w| Placement::new(w.code.clone(), w.code.clone(), width))
                        })
                        .collect()
                })
                .collect(),
        )
    }
}

/// Carries an operator's dashboard from the old preference to the home
/// board's, once: the old one is removed whether or not it could be read.
pub(crate) async fn carry_legacy(db: &Db, user_id: i64) {
    let Ok(Some(json)) =
        laterite_auth::store::user_preference(db, user_id, LEGACY_PREFERENCE).await
    else {
        return;
    };
    if let Ok(old) = serde_json::from_str::<LegacyLayout>(&json) {
        if let Ok(new) = serde_json::to_string(&old.converted()) {
            let _ = laterite_auth::store::set_user_preference(
                db,
                user_id,
                &format!("board.{HOME}"),
                &new,
            )
            .await;
        }
    }
    let _ = laterite_auth::store::clear_user_preference(db, user_id, LEGACY_PREFERENCE).await;
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
    fn sizes_flow_into_rows_that_fill_twenty_columns() {
        use Size::*;
        assert_eq!(
            flow(&[Third, Third, Third, Third]),
            vec![vec![7, 7, 6], vec![7]]
        );
        assert_eq!(
            flow(&[Half, Half, Full, Half, Third]),
            vec![vec![10, 10], vec![20], vec![10, 10]]
        );
        assert_eq!(flow(&[Third, Half, Third]), vec![vec![10, 10], vec![7]]);
        assert!(flow(&[Half, Third, Third, Full]).iter().all(|r| r
            .iter()
            .map(|w| u16::from(*w))
            .sum::<u16>()
            <= 20));
    }

    #[test]
    fn an_old_dashboard_becomes_a_board_layout_and_the_default_follows_the_order() {
        let old: LegacyLayout = serde_json::from_str(
            r#"{"widgets":[{"code":"backend.status","size":"full"},{"code":"acme.sales"},{"code":"backend.welcome","size":"third"}]}"#,
        )
        .unwrap();
        let rows = old.converted().rows;
        assert_eq!(rows.len(), 2);
        assert_eq!(
            (rows[0][0].id.as_str(), rows[0][0].width),
            ("backend.status", 20)
        );
        assert_eq!(
            rows[1]
                .iter()
                .map(|p| (p.kind.as_str(), p.width))
                .collect::<Vec<_>>(),
            vec![("acme.sales", 10), ("backend.welcome", 10)]
        );
        assert_eq!(Size::of_width(7), Size::Third);
        assert_eq!(Size::of_width(10), Size::Half);
        assert_eq!(Size::of_width(20), Size::Full);
    }
}
