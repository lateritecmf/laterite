# Boards

A board is a dashboard a screen hosts: widgets on a twenty-column grid, each
fetched after the page opens, arranged by the people who may.

## Host a board

```rust
use laterite_admin::board::{Board, BoardCx, Layout, Placement};
use laterite_admin::routes::{RouteCtx, Screen};
use laterite_admin::{AuthenticatedUser, PageHead, Shell};

struct Sales;

impl Screen for Sales {
    fn mount(&self, ctx: &RouteCtx) -> Router {
        let board = Arc::new(
            Board::new(
                "acme.sales",
                Layout::rows(vec![
                    vec![Placement::new("orders", "acme.orders_today", 7).locked()],
                    vec![Placement::new("trend", "acme.revenue", 20)
                        .properties(json!({ "days": "30" }))],
                ]),
            )
            .edit_permission("acme.view_sales")
            .share_permission("acme.manage_sales")
            .offer(["acme.*"]),
        );
        let (page, ctx, base) = (board.clone(), ctx.clone(), ctx.base_path().to_string());
        Router::new()
            .route("/", get(move |Extension(shell): Extension<Shell>,
                                  Extension(user): Extension<AuthenticatedUser>,
                                  Query(query): Query<HashMap<String, String>>| {
                let (board, ctx, base) = (page.clone(), ctx.clone(), base.clone());
                async move {
                    let cx = BoardCx { ctx: &ctx, shell: &shell, user: &user, query: &query, width: 20 };
                    let body = board.body(&cx, &base).await;
                    shell.page_with(PageHead::new(t!("Sales")).assets(&board.assets(&ctx)), body)
                }
            }))
            .merge(board.routes(ctx))
    }
}
```

`Board` method | Sets | Default
--- | --- | ---
`new(key, layout)` | the key its layouts are kept under, and the widgets it starts with | required
`editing(Editing)` | `Off`, `Personal`, `Shared` or `Both` | `Both`
`edit_permission(p)` | who arranges their own | whoever sees the board
`share_permission(p)` | who changes the layout everyone sees | nobody
`allow(Allow)` | which of moving, resizing, adding, removing and configuring edit mode offers | all
`offer(codes)` | the kinds Add widget lists; `acme.*` is every kind under a prefix | every registered kind
`with_kind(kind)` | a kind this board alone offers | none
`rows()` | each row on its own line | a widget sits under the one above it in its columns
`carry(names)` | query parameters the board's requests keep, such as `range` | none

`Placement` method | Sets
--- | ---
`new(id, kind, width)` | the widget's id on this board, its kind's code, its columns of 20
`properties(json)` | its settings
`locked()` | not moved, resized or removed
`keep()` | no Remove
`fixed_settings()` | no Configure

## Write a kind of widget

```rust
use laterite_admin::board::{BoardCx, Kind, KindReg};
use laterite_admin::form::FormField;

struct Revenue;

#[laterite_core::strata::async_trait]
impl Kind for Revenue {
    fn code(&self) -> &str {
        "acme.revenue"
    }
    fn label(&self) -> Text {
        t!("Revenue")
    }
    fn group(&self) -> Text {
        t!("Sales")
    }
    fn default_width(&self) -> u8 {
        10
    }
    fn properties(&self) -> Vec<FormField> {
        vec![FormField::of("days", t!("Days"), "select")
            .options(json!({ "options": [{ "value": "7", "label": "7" }, { "value": "30", "label": "30" }] }))]
    }
    fn title(&self, shell: &Shell, properties: &Value) -> Option<String> {
        Some(shell.t("Revenue"))
    }
    async fn render(&self, cx: &BoardCx<'_>, properties: &Value) -> Result<Markup, String> {
        let days = properties["days"].as_str().unwrap_or("30");
        acme::revenue::chart(cx.ctx.db(), days).await.map_err(|e| e.to_string())
    }
}

fn register(&self, registry: &mut laterite_core::Registry) {
    registry.add(KindReg::new(Arc::new(Revenue)));
}
```

`Kind` method | Returns | Default
--- | --- | ---
`code` | `vendor.name`, unique across modules | required
`label`, `group`, `description` | its entry in Add widget | group `Widgets`, no description
`permission` | the permission needed to see or add it | none
`default_width`, `min_width`, `max_width` | columns of 20 | 7, 3, 20
`resizable` | whether its width changes | `true`
`once` | at most one on a board | `false`
`bare` | drawn with no card around it | `false`
`properties` | its settings as form fields; with none it has no Configure | none
`title` | the heading above it | none
`refresh` | seconds between fetches | fetched once
`render` | the content as `Markup`; an `Err` is shown in the widget's place | required

`BoardCx` gives `ctx`, `shell`, `user`, `query` (the page's query string) and `width`.
