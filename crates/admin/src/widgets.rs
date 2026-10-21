//! The widgets the framework ships on the dashboard: a welcome, the sections
//! an operator may open, recent activity from the audit log, and the system's
//! status. Each is an ordinary [`Widget`], registered like a module's own.

use crate::html::Markup;
use crate::widget::{Size, Widget, WidgetCx};
use askama::Template;
use laterite_core::arrange::{slot, Position};
use laterite_core::{t, Text};
use std::sync::Arc;

pub(crate) fn builtin() -> Vec<crate::widget::WidgetReg> {
    vec![
        crate::widget::WidgetReg::new(Arc::new(Welcome)).position(Position::new().slot(slot::HOME)),
        crate::widget::WidgetReg::new(Arc::new(Manage))
            .position(Position::new().slot(slot::HOME).order(10)),
        crate::widget::WidgetReg::new(Arc::new(Activity))
            .position(Position::new().slot(slot::SYSTEM)),
        crate::widget::WidgetReg::new(Arc::new(Status))
            .position(Position::new().slot(slot::SYSTEM).order(10)),
    ]
}

/// Who is signed in.
pub struct Welcome;

#[derive(Template)]
#[template(path = "widgets/welcome.html")]
struct WelcomeTmpl {
    full_name: String,
    signed_in_as: String,
}

#[laterite_core::strata::async_trait]
impl Widget for Welcome {
    fn code(&self) -> &str {
        "backend.welcome"
    }
    fn label(&self) -> Text {
        t!("Welcome")
    }
    fn description(&self) -> Option<Text> {
        Some(t!("Who is signed in."))
    }
    async fn render(&self, cx: &WidgetCx<'_>) -> Result<Markup, String> {
        Markup::from_template(&WelcomeTmpl {
            full_name: cx.shell.full_name.clone(),
            signed_in_as: cx.shell.tfs(
                "Signed in as {name}.",
                &[("name", cx.user.user.username.as_str())],
            ),
        })
        .map_err(|e| e.to_string())
    }
}

/// The sections this operator may open, as cards.
pub struct Manage;

#[derive(Template)]
#[template(path = "widgets/manage.html")]
struct ManageTmpl {
    cards: Vec<(String, String, String)>,
}

#[laterite_core::strata::async_trait]
impl Widget for Manage {
    fn code(&self) -> &str {
        "backend.manage"
    }
    fn label(&self) -> Text {
        t!("Manage")
    }
    fn description(&self) -> Option<Text> {
        Some(t!("A card for every section you may open."))
    }
    fn size(&self) -> Size {
        Size::Full
    }
    async fn render(&self, cx: &WidgetCx<'_>) -> Result<Markup, String> {
        let cards = cx
            .shell
            .nav
            .iter()
            .filter(|n| n.path != cx.shell.base)
            .map(|n| {
                (
                    n.label.clone(),
                    n.path.clone(),
                    cx.shell.tfs("Open {label}", &[("label", n.label.as_str())]),
                )
            })
            .collect();
        Markup::from_template(&ManageTmpl { cards }).map_err(|e| e.to_string())
    }
}

/// The latest entries of the audit log.
pub struct Activity;

#[derive(Template)]
#[template(path = "widgets/activity.html")]
struct ActivityTmpl {
    rows: Vec<ActivityRow>,
    empty: String,
    all: String,
    all_href: String,
}

struct ActivityRow {
    when: String,
    who: String,
    action: String,
    target: String,
}

#[laterite_core::strata::async_trait]
impl Widget for Activity {
    fn code(&self) -> &str {
        "backend.activity"
    }
    fn label(&self) -> Text {
        t!("Recent activity")
    }
    fn description(&self) -> Option<Text> {
        Some(t!("The latest changes from the audit log."))
    }
    fn permission(&self) -> Option<&str> {
        Some("backend.view_audit_log")
    }
    fn refresh(&self) -> Option<u32> {
        Some(60)
    }
    async fn render(&self, cx: &WidgetCx<'_>) -> Result<Markup, String> {
        let limit = match cx.size {
            Size::Full => 12,
            _ => 8,
        };
        let records = cx
            .state
            .auth
            .recent_audit(limit)
            .await
            .map_err(|e| e.to_string())?;
        let rows = records
            .into_iter()
            .map(|r| ActivityRow {
                when: cx.shell.datetime(&r.created_at.to_rfc3339()),
                who: r.actor_username,
                action: r.action,
                target: r
                    .target_label
                    .or(r.target_id)
                    .map(|t| match &r.target_type {
                        Some(kind) => format!("{kind} {t}"),
                        None => t,
                    })
                    .unwrap_or_default(),
            })
            .collect();
        Markup::from_template(&ActivityTmpl {
            rows,
            empty: cx.t(&t!("Nothing recorded yet.")),
            all: cx.t(&t!("Open the audit log")),
            all_href: format!("{}/audit-log", cx.base()),
        })
        .map_err(|e| e.to_string())
    }
}

/// The framework and the database behind this panel.
pub struct Status;

#[derive(Template)]
#[template(path = "widgets/status.html")]
struct StatusTmpl {
    facts: Vec<(String, String)>,
}

#[laterite_core::strata::async_trait]
impl Widget for Status {
    fn code(&self) -> &str {
        "backend.status"
    }
    fn label(&self) -> Text {
        t!("System")
    }
    fn description(&self) -> Option<Text> {
        Some(t!(
            "The framework version and the database behind this panel."
        ))
    }
    fn permission(&self) -> Option<&str> {
        Some("backend.manage_plugins")
    }
    fn size(&self) -> Size {
        Size::Third
    }
    async fn render(&self, cx: &WidgetCx<'_>) -> Result<Markup, String> {
        let backend = match cx.db().backend {
            laterite_core::DbBackend::Postgres => "PostgreSQL",
            laterite_core::DbBackend::Mysql => "MySQL",
            laterite_core::DbBackend::Sqlite => "SQLite",
        };
        let facts = vec![
            (
                cx.t(&t!("Framework")),
                format!("Laterite {}", env!("CARGO_PKG_VERSION")),
            ),
            (cx.t(&t!("Database")), backend.to_string()),
            (cx.t(&t!("Timezone")), cx.state.timezone.name().to_string()),
        ];
        Markup::from_template(&StatusTmpl { facts }).map_err(|e| e.to_string())
    }
}
