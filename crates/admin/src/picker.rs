//! The reference-picker seam: a generic field picks a value from a data source
//! (another table, a hierarchy) without the framework knowing the source.
//!
//! A [`PickerSource`] is a domain service the picker field and its QUERY
//! endpoints call: it owns no route and returns [`PickerNode`]s, never HTML.
//! Sources are contributed to a [`PickerRegistry`] and selected by a field's
//! `{"source": "vendor.name"}` option. [`TableSource`] is a built-in source over
//! a single table, so a simple reference needs no code; a hierarchy or a computed
//! label implements the trait directly.
//!
//! Search and resolve are exposed as two routes under `{admin}/pickers/{source}`,
//! served with the QUERY method (reads carry no CSRF token; the session cookie
//! and, optionally, a per-source permission gate them).

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{header, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use laterite_auth::AuthenticatedUser;
use laterite_core::query::{bind_values, build as to_sql, text_cast};
use laterite_core::strata::async_trait;
use laterite_core::{AnyRowExt, Db};
use sea_query::{Alias, Expr, Func, Query, SelectStatement};
use serde::{Deserialize, Serialize};

use crate::sql::valid_ident;
use crate::AdminState;

/// One candidate or resolved node. `id` is a string: the generic form layer
/// treats stored values as text, and a JSON number would corrupt an integer id
/// past 2^53 in the browser, so a picker id is never serialised as a number.
#[derive(Debug, Clone, Serialize)]
pub struct PickerNode {
    pub id: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// A picker source failed to search or resolve.
#[derive(Debug, thiserror::Error)]
#[error("picker source error: {0}")]
pub struct PickerError(pub String);

/// A source of picker candidates: a domain service the picker field and its
/// endpoints call. Implementors build SQL in a scoped block and drop the builder
/// before awaiting (sea-query identifiers are not `Send`), as the store queries
/// elsewhere do.
#[async_trait]
pub trait PickerSource: Send + Sync + 'static {
    /// Candidates matching `q`, capped at `limit` (the caller clamps it).
    async fn search(&self, db: &Db, q: &str, limit: u32) -> Result<Vec<PickerNode>, PickerError>;
    /// The node a stored `id` refers to, or `None` if it no longer exists.
    async fn resolve(&self, db: &Db, id: &str) -> Result<Option<PickerNode>, PickerError>;
}

/// A registered source: its dotted `vendor.name` key, the source, and an optional
/// permission gating its endpoints (a source over sensitive data sets one).
pub struct PickerSourceReg {
    pub name: String,
    pub source: Arc<dyn PickerSource>,
    pub permission: Option<String>,
}

impl PickerSourceReg {
    /// A source readable by any signed-in operator.
    pub fn new(name: impl Into<String>, source: Arc<dyn PickerSource>) -> Self {
        Self {
            name: name.into(),
            source,
            permission: None,
        }
    }
    /// Gates this source's endpoints on a permission.
    pub fn require(mut self, permission: impl Into<String>) -> Self {
        self.permission = Some(permission.into());
        self
    }
}

/// The picker-source registry, keyed by source name.
pub type PickerRegistry = HashMap<String, PickerSourceReg>;

/// A built-in [`PickerSource`] over a single table: candidates are rows whose
/// `label_col` matches (case-insensitive substring), and a stored id resolves by
/// `id_col`. `hint_col`, when set, supplies each node's disambiguating hint. A
/// hierarchy or a computed label implements [`PickerSource`] directly instead.
pub struct TableSource {
    table: String,
    id_col: String,
    label_col: String,
    hint_col: Option<String>,
}

impl TableSource {
    pub fn new(
        table: impl Into<String>,
        id_col: impl Into<String>,
        label_col: impl Into<String>,
    ) -> Self {
        Self {
            table: table.into(),
            id_col: id_col.into(),
            label_col: label_col.into(),
            hint_col: None,
        }
    }

    /// Sets the column supplying each node's disambiguating hint.
    pub fn with_hint(mut self, hint_col: impl Into<String>) -> Self {
        self.hint_col = Some(hint_col.into());
        self
    }

    /// Guards the descriptor's identifiers (defence in depth; sea-query quotes
    /// them regardless).
    fn idents_valid(&self) -> bool {
        valid_ident(&self.table)
            && valid_ident(&self.id_col)
            && valid_ident(&self.label_col)
            && self.hint_col.as_deref().is_none_or(valid_ident)
    }

    /// The `id`, `label`, and (when configured) `hint` columns cast to text, from
    /// the table. The where clause and limit are added per query.
    fn base_select(&self, cast: &str) -> SelectStatement {
        let mut sel = Query::select();
        sel.expr_as(
            Expr::col(Alias::new(&self.id_col)).cast_as(Alias::new(cast)),
            Alias::new("id"),
        );
        sel.expr_as(
            Expr::col(Alias::new(&self.label_col)).cast_as(Alias::new(cast)),
            Alias::new("label"),
        );
        if let Some(hint) = &self.hint_col {
            sel.expr_as(
                Expr::col(Alias::new(hint)).cast_as(Alias::new(cast)),
                Alias::new("hint"),
            );
        }
        sel.from(Alias::new(&self.table));
        sel
    }
}

#[async_trait]
impl PickerSource for TableSource {
    async fn search(&self, db: &Db, q: &str, limit: u32) -> Result<Vec<PickerNode>, PickerError> {
        if !self.idents_valid() {
            return Err(PickerError("invalid source columns".into()));
        }
        let has_hint = self.hint_col.is_some();
        let (sql, values) = {
            let cast = text_cast(db.backend);
            let mut sel = self.base_select(cast);
            let pattern = format!("%{}%", q.to_lowercase());
            sel.and_where(
                Expr::expr(Func::lower(Expr::col(Alias::new(&self.label_col)))).like(pattern),
            );
            sel.limit(limit as u64);
            to_sql(db.backend, sel)
        };
        let rows = bind_values(sqlx::query(&sql), values)
            .fetch_all(&db.pool)
            .await
            .map_err(|e| PickerError(e.to_string()))?;
        Ok(rows
            .iter()
            .map(|r| PickerNode {
                id: r.get_text_opt("id").ok().flatten().unwrap_or_default(),
                label: r.get_text_opt("label").ok().flatten().unwrap_or_default(),
                hint: has_hint
                    .then(|| r.get_text_opt("hint").ok().flatten())
                    .flatten(),
            })
            .collect())
    }

    async fn resolve(&self, db: &Db, id: &str) -> Result<Option<PickerNode>, PickerError> {
        if !self.idents_valid() {
            return Err(PickerError("invalid source columns".into()));
        }
        let has_hint = self.hint_col.is_some();
        let (sql, values) = {
            let cast = text_cast(db.backend);
            let mut sel = self.base_select(cast);
            sel.and_where(
                Expr::col(Alias::new(&self.id_col))
                    .cast_as(Alias::new(cast))
                    .eq(id),
            );
            sel.limit(1);
            to_sql(db.backend, sel)
        };
        let row = bind_values(sqlx::query(&sql), values)
            .fetch_optional(&db.pool)
            .await
            .map_err(|e| PickerError(e.to_string()))?;
        Ok(row.map(|r| PickerNode {
            id: r.get_text_opt("id").ok().flatten().unwrap_or_default(),
            label: r.get_text_opt("label").ok().flatten().unwrap_or_default(),
            hint: has_hint
                .then(|| r.get_text_opt("hint").ok().flatten())
                .flatten(),
        }))
    }
}

// The QUERY endpoints. The wire contract (frozen here and consumed by the
// picker widget): search takes `{"q","limit"?}` and returns `{"items":[...]}`;
// resolve takes `{"id"}` and returns `{"item": node|null}`. Unknown source is
// 404, a non-QUERY method is 405, and both responses are `no-store`.

#[derive(Deserialize)]
struct SearchReq {
    #[serde(default)]
    q: String,
    #[serde(default)]
    limit: Option<u32>,
}

#[derive(Deserialize)]
struct ResolveReq {
    #[serde(default)]
    id: String,
}

#[derive(Serialize)]
struct SearchResp {
    items: Vec<PickerNode>,
}

#[derive(Serialize)]
struct ResolveResp {
    item: Option<PickerNode>,
}

const DEFAULT_LIMIT: u32 = 20;
const MAX_LIMIT: u32 = 50;

pub(crate) async fn search(
    method: Method,
    State(state): State<AdminState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(source): Path<String>,
    body: Bytes,
) -> Response {
    let reg = match guard(&method, &state, &user, &source) {
        Ok(reg) => reg,
        Err(resp) => return *resp,
    };
    let req: SearchReq = match serde_json::from_slice(&body) {
        Ok(req) => req,
        Err(_) => return json_no_store(StatusCode::BAD_REQUEST, &err("invalid request body")),
    };
    let limit = req.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    match reg.source.search(&state.db, req.q.trim(), limit).await {
        Ok(items) => json_no_store(StatusCode::OK, &SearchResp { items }),
        Err(_) => json_no_store(StatusCode::INTERNAL_SERVER_ERROR, &err("search failed")),
    }
}

pub(crate) async fn resolve(
    method: Method,
    State(state): State<AdminState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(source): Path<String>,
    body: Bytes,
) -> Response {
    let reg = match guard(&method, &state, &user, &source) {
        Ok(reg) => reg,
        Err(resp) => return *resp,
    };
    let req: ResolveReq = match serde_json::from_slice(&body) {
        Ok(req) => req,
        Err(_) => return json_no_store(StatusCode::BAD_REQUEST, &err("invalid request body")),
    };
    match reg.source.resolve(&state.db, req.id.trim()).await {
        Ok(item) => json_no_store(StatusCode::OK, &ResolveResp { item }),
        Err(_) => json_no_store(StatusCode::INTERNAL_SERVER_ERROR, &err("resolve failed")),
    }
}

/// Enforces QUERY, resolves the named source, and checks its permission. Returns
/// the ready response on any failure so the handler just early-returns it.
fn guard<'a>(
    method: &Method,
    state: &'a AdminState,
    user: &AuthenticatedUser,
    source: &str,
) -> Result<&'a PickerSourceReg, Box<Response>> {
    if method.as_str() != "QUERY" {
        return Err(Box::new(json_no_store(
            StatusCode::METHOD_NOT_ALLOWED,
            &err("use the QUERY method"),
        )));
    }
    let reg = state.pickers.get(source).ok_or_else(|| {
        Box::new(json_no_store(
            StatusCode::NOT_FOUND,
            &err("unknown picker source"),
        ))
    })?;
    if let Some(permission) = &reg.permission {
        if !user.permissions.allows(permission) {
            return Err(Box::new(json_no_store(
                StatusCode::FORBIDDEN,
                &err("forbidden"),
            )));
        }
    }
    Ok(reg)
}

fn json_no_store<T: Serialize>(status: StatusCode, body: &T) -> Response {
    (status, [(header::CACHE_CONTROL, "no-store")], Json(body)).into_response()
}

fn err(message: &str) -> serde_json::Value {
    serde_json::json!({ "error": message })
}

#[cfg(test)]
mod tests {
    use super::*;
    use laterite_core::strata::{
        async_trait as strata_async_trait, ColumnDef, CoreResult, Migration, MigrationSet, Schema,
        Table,
    };
    use laterite_core::testing::{connect_test, TestGuard};

    struct CreatePlaces;
    #[strata_async_trait(?Send)]
    impl Migration for CreatePlaces {
        fn name(&self) -> &str {
            "0001_create_places"
        }
        async fn up(&self, s: &mut Schema<'_>) -> CoreResult<()> {
            s.exec(
                Table::create()
                    .table(Alias::new("places"))
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Alias::new("id"))
                            .big_integer()
                            .not_null()
                            .auto_increment()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Alias::new("name")).text().not_null())
                    .col(ColumnDef::new(Alias::new("region")).text().not_null())
                    .to_owned(),
            )
            .await
        }
    }

    async fn test_db() -> (Db, TestGuard) {
        let places = MigrationSet::new("test.places", vec![Box::new(CreatePlaces)]);
        connect_test(&[places]).await
    }

    async fn insert(db: &Db, name: &str, region: &str) {
        let (sql, values) = to_sql(
            db.backend,
            Query::insert()
                .into_table(Alias::new("places"))
                .columns([Alias::new("name"), Alias::new("region")])
                .values_panic([name.to_string().into(), region.to_string().into()])
                .to_owned(),
        );
        bind_values(sqlx::query(&sql), values)
            .execute(&db.pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn table_source_searches_case_insensitively_and_resolves_by_id() {
        let (db, _guard) = test_db().await;
        insert(&db, "Kigali", "Rwanda").await;
        insert(&db, "Kampala", "Uganda").await;
        let source = TableSource::new("places", "id", "name").with_hint("region");

        // Case-insensitive substring: `kig` finds `Kigali`, not `Kampala`.
        let hits = source.search(&db, "kig", 10).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].label, "Kigali");
        assert_eq!(hits[0].hint.as_deref(), Some("Rwanda"));

        // The stored id round-trips to its node.
        let node = source
            .resolve(&db, &hits[0].id)
            .await
            .unwrap()
            .expect("id resolves");
        assert_eq!(node.label, "Kigali");

        // A missing id resolves to None (a deleted referent).
        assert!(source.resolve(&db, "99999").await.unwrap().is_none());
    }
}
