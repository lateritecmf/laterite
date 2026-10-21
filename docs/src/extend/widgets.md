# Dashboard Widgets

```rust
use laterite_admin::widget::{Size, Widget, WidgetCx, WidgetReg};
use laterite_admin::{html::Markup, AdminRegistry};
use laterite_core::arrange::{slot, Position};
use laterite_core::{t, Text};
use std::sync::Arc;

struct Sales;

#[laterite_core::strata::async_trait]
impl Widget for Sales {
    fn code(&self) -> &str {
        "acme.sales"
    }
    fn label(&self) -> Text {
        t!("Sales today")
    }
    fn description(&self) -> Option<Text> {
        Some(t!("Orders and revenue since midnight."))
    }
    fn permission(&self) -> Option<&str> {
        Some("acme.view_sales")
    }
    fn size(&self) -> Size {
        Size::Third
    }
    fn refresh(&self) -> Option<u32> {
        Some(30)
    }
    async fn render(&self, cx: &WidgetCx<'_>) -> Result<Markup, String> {
        let count = acme::orders::today(cx.db()).await.map_err(|e| e.to_string())?;
        Ok(Markup::raw_unchecked(format!("<p class=\"lat-widget__name\">{count}</p>")))
    }
}

fn register(&self, registry: &mut laterite_core::Registry) {
    registry.add_widget(
        WidgetReg::new(Arc::new(Sales)).position(Position::new().slot(slot::APPLICATION)),
    );
}
```

Method | Returns | Default
--- | --- | ---
`code` | `vendor.name`, unique across modules | required
`label` | the heading, a `Text` | required
`description` | a line under the label in Manage widgets | none
`permission` | the permission an operator needs to see it | shown to every operator
`size` | `Size::Third`, `Size::Half`, `Size::Full` | `Half`
`refresh` | seconds between fetches of the body | rendered once with the page
`render` | the body as `Markup`; an `Err` shows a notice inside the frame | required

`WidgetCx` gives `db()`, `base()`, `t(&Text)`, `shell`, `user` and `size`.

## Arrangement

```toml
[backend.order]
"backend.dashboard" = ["acme.sales", "backend.welcome"]
```

Widgets sit in the `backend.dashboard` hierarchy: `slot::HOME` holds the welcome and manage cards, `slot::SYSTEM` the activity and status; a module's widget lands in `slot::APPLICATION` unless its position says otherwise. A deployment reorders with `[backend.order]`; every operator chooses, sizes and orders their own under Manage widgets, stored in their preferences.

## Built in

Code | Shows | Permission
--- | --- | ---
`backend.welcome` | the operator's name | none
`backend.manage` | a card per section they may open | none
`backend.activity` | the latest audit-log entries, refreshed every minute | `backend.view_audit_log`
`backend.status` | the framework version, database and timezone | `backend.manage_plugins`

A module registering a widget under a built-in's code replaces it.

## Fragment

```
GET /admin/dashboard/widget/acme.sales?size=third
```

The body alone, as the refresh interval fetches it; a widget the operator may not see answers 403.
