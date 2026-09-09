//! Exporting a list to a file.
//!
//! The export runs the **same query the screen just ran**: the operator's chosen
//! columns, the active search, the filters and the sort. What downloads is what
//! they were looking at, minus the paging, so an export never quietly disagrees
//! with the screen that offered it.
//!
//! Rows are read a page at a time and written into one body. An export larger
//! than [`MAX_ROWS`] is refused rather than truncated: a short file that does not
//! say it is short is worse than no file.
//!
//! CSV is headed by the column **labels**, localized, because a spreadsheet is
//! read by a person. JSON is keyed by the column **names**, because it is read by
//! a script that wants a stable key. The reference system heads its CSV the same
//! way.

use std::collections::HashMap;

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use laterite_auth::AuthenticatedUser;
use laterite_core::t;

use crate::list::{self, ListConfig, ListQuery};
use crate::session::{FlashLevel, SessionHandle};
use crate::AdminState;

/// The most rows one export may carry. Beyond this the operator is asked to
/// narrow the list, because the whole body is built in memory.
pub(crate) const MAX_ROWS: i64 = 20_000;

/// How many rows are read per round trip while building the body.
const PAGE: i64 = 1_000;

/// Which shape the file takes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    Csv,
    Json,
}

impl Format {
    /// Reads the requested format, defaulting to CSV. An unknown value is CSV
    /// too: a download is not the place to argue about a query parameter.
    fn read(raw: &HashMap<String, String>) -> Self {
        match raw.get("format").map(String::as_str) {
            Some("json") => Format::Json,
            _ => Format::Csv,
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Format::Csv => "csv",
            Format::Json => "json",
        }
    }

    fn content_type(self) -> &'static str {
        match self {
            Format::Csv => "text/csv; charset=utf-8",
            Format::Json => "application/json",
        }
    }
}

/// Streams the current list view to a file.
pub(crate) async fn handle(
    State(state): State<AdminState>,
    config: &ListConfig,
    path: &str,
    raw: &HashMap<String, String>,
    user: &AuthenticatedUser,
    shell: &crate::Shell,
    session: &SessionHandle,
) -> Response {
    let format = Format::read(raw);
    let back = format!("{}{}", state.admin_path, path);

    // The same narrowing the screen applies, so the file has the columns the
    // operator can see and nothing else.
    let stored = list::stored_columns(&state, path, user).await;
    let config = list::narrowed(config, stored.as_deref());

    let (order_by, order_dir) = list::resolve_sort(
        &config,
        raw.get("sort").map(String::as_str),
        raw.get("dir").map(String::as_str),
    );
    let filters = list::resolve_filters(&config, raw);
    let q = raw.get("q").map(String::as_str).unwrap_or("").trim();

    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut offset = 0i64;
    loop {
        let req = ListQuery {
            offset,
            order_by: &order_by,
            order_dir,
            q,
            filters: &filters,
            per_page: Some(PAGE),
        };
        let page = match list::query(&state.db, &config, &req).await {
            Ok(page) => page,
            Err(e) => {
                tracing::error!(entity = %config.entity, error = %e, "export query failed");
                session.push_flash(FlashLevel::Error, t!("The export could not be produced."));
                return axum::response::Redirect::to(&back).into_response();
            }
        };
        if page.total > MAX_ROWS {
            session.push_flash(
                FlashLevel::Error,
                t!("That is too many records to export at once. Narrow the list and try again."),
            );
            return axum::response::Redirect::to(&back).into_response();
        }
        let count = page.rows.len();
        rows.extend(page.rows.into_iter().map(|r| r.cells));
        if count < PAGE as usize {
            break;
        }
        offset += PAGE;
    }

    // Labels for a person, names for a script.
    let labels: Vec<String> = config.columns.iter().map(|c| shell.tt(&c.label)).collect();
    let names: Vec<String> = config.columns.iter().map(|c| c.field.clone()).collect();
    let body = match format {
        Format::Csv => match to_csv(&labels, &rows) {
            Ok(body) => body,
            Err(e) => {
                tracing::error!(error = %e, "writing the export failed");
                session.push_flash(FlashLevel::Error, t!("The export could not be produced."));
                return axum::response::Redirect::to(&back).into_response();
            }
        },
        Format::Json => to_json(&names, &rows),
    };

    let filename = format!("{}.{}", config.entity, format.extension());
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, format.content_type().to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            ),
        ],
        body,
    )
        .into_response()
}

/// The rows as CSV, quoted and escaped by the `csv` crate rather than by hand.
fn to_csv(headers: &[String], rows: &[Vec<String>]) -> Result<String, csv::Error> {
    let mut writer = csv::Writer::from_writer(Vec::new());
    writer.write_record(headers)?;
    for row in rows {
        writer.write_record(row)?;
    }
    let bytes = writer.into_inner().map_err(|e| e.into_error())?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// The rows as an array of objects keyed by column, which is what a consumer of
/// a list export expects to iterate.
fn to_json(headers: &[String], rows: &[Vec<String>]) -> String {
    let out: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            let pairs: serde_json::Map<String, serde_json::Value> = headers
                .iter()
                .zip(row)
                .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                .collect();
            serde_json::Value::Object(pairs)
        })
        .collect();
    serde_json::to_string_pretty(&out).unwrap_or_else(|_| "[]".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<Vec<String>> {
        vec![
            vec!["ada".to_string(), "Yes".to_string()],
            // The values a hand-rolled writer gets wrong: a comma, a quote, a newline.
            vec!["b, comma".to_string(), "say \"hi\"".to_string()],
            vec!["line\nbreak".to_string(), "No".to_string()],
        ]
    }

    fn headers() -> Vec<String> {
        vec!["username".to_string(), "superuser".to_string()]
    }

    use laterite_core::testing::connect_test;
    use laterite_core::Db;

    async fn seeded() -> (Db, laterite_core::testing::TestGuard) {
        let (db, guard) = connect_test(&[laterite_auth::migrations()]).await;
        for (name, superuser) in [("ada", true), ("brendan", false), ("clara", false)] {
            let hash = laterite_auth::password::hash_password("x").unwrap();
            laterite_auth::store::create_user(
                &db,
                name,
                &format!("{name}@example.test"),
                name,
                None,
                &hash,
                superuser,
            )
            .await
            .unwrap();
        }
        (db, guard)
    }

    fn users_list() -> ListConfig {
        ListConfig {
            entity: "backend_users".to_string(),
            title: "Users".into(),
            columns: vec![
                crate::list::ListColumn::new("username", "Username"),
                crate::list::ListColumn::new("is_superuser", "Superuser").yes_no(),
            ],
            order_by: "username".to_string(),
            order_dir: crate::list::SortDir::Asc,
            filters: vec![crate::list::ListFilter::boolean(
                "is_superuser",
                "Superuser",
            )],
            ..Default::default()
        }
    }

    async fn body_of(raw: HashMap<String, String>) -> (String, String) {
        let (db, _guard) = seeded().await;
        let state = AdminState::new(
            laterite_auth::AuthService::new(db.clone(), laterite_auth::AuthConfig::default()),
            db,
        );
        let resp = handle(
            State(state),
            &users_list(),
            "/users",
            &raw,
            &crate::audit::test_actor(),
            &crate::Shell::test(),
            &crate::session::SessionHandle::from_blob(None),
        )
        .await;
        let disposition = resp
            .headers()
            .get(header::CONTENT_DISPOSITION)
            .map(|v| v.to_str().unwrap().to_string())
            .unwrap_or_default();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        (String::from_utf8(bytes.to_vec()).unwrap(), disposition)
    }

    /// The export is the screen's query, not the whole table: the same search,
    /// filter and sort the operator was looking at.
    #[tokio::test]
    async fn the_file_matches_the_view_that_produced_it() {
        let (all, disposition) = body_of(HashMap::new()).await;
        assert!(disposition.contains("backend_users.csv"), "{disposition}");
        // Header row plus every seeded user, ordered as the descriptor says.
        let lines: Vec<&str> = all.trim().lines().collect();
        // Headed by the labels a person reads, not the column names.
        assert_eq!(lines[0], "Username,Superuser");
        assert_eq!(lines.len(), 4);
        assert!(
            lines[1].starts_with("ada"),
            "sorted ascending: {}",
            lines[1]
        );

        // A filter narrows the file exactly as it narrows the screen.
        let mut raw = HashMap::new();
        raw.insert("f_is_superuser".to_string(), "1".to_string());
        let (filtered, _) = body_of(raw).await;
        assert_eq!(filtered.trim().lines().count(), 2, "header plus one row");
        assert!(filtered.contains("ada"));
        assert!(!filtered.contains("brendan"));

        // So does a search.
        let mut raw = HashMap::new();
        raw.insert("q".to_string(), "bren".to_string());
        let (searched, _) = body_of(raw).await;
        assert_eq!(searched.trim().lines().count(), 2);
        assert!(searched.contains("brendan"));
    }

    #[tokio::test]
    async fn json_asks_for_the_same_rows_in_the_other_shape() {
        let mut raw = HashMap::new();
        raw.insert("format".to_string(), "json".to_string());
        let (body, disposition) = body_of(raw).await;
        assert!(disposition.contains("backend_users.json"), "{disposition}");
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed.as_array().unwrap().len(), 3);
        // Keyed by column name, which is the stable key a script wants.
        assert_eq!(parsed[0]["username"], "ada");
    }

    #[test]
    fn csv_quotes_and_escapes_every_awkward_value() {
        let out = to_csv(&headers(), &rows()).unwrap();
        assert!(out.starts_with("username,superuser\n"));
        assert!(out.contains("\"b, comma\""), "a comma is quoted: {out}");
        assert!(
            out.contains("\"say \"\"hi\"\"\""),
            "a quote is doubled: {out}"
        );
        assert!(
            out.contains("\"line\nbreak\""),
            "a newline is quoted: {out}"
        );
    }

    #[test]
    fn json_is_an_array_of_objects_keyed_by_column() {
        let out = to_json(&headers(), &rows());
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed.as_array().unwrap().len(), 3);
        assert_eq!(parsed[0]["username"], "ada");
        assert_eq!(parsed[1]["superuser"], "say \"hi\"");
    }

    #[test]
    fn the_format_defaults_to_csv() {
        let mut raw = HashMap::new();
        assert!(Format::read(&raw) == Format::Csv);
        raw.insert("format".to_string(), "json".to_string());
        assert!(Format::read(&raw) == Format::Json);
        // An unrecognised value downloads rather than erroring.
        raw.insert("format".to_string(), "xlsx".to_string());
        assert!(Format::read(&raw) == Format::Csv);
    }
}
