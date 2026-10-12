//! Related records on a form: a resource declares which other entity's rows
//! belong to each of its records, and the edit page carries a section for them.
//!
//! A section is the list engine scoped to the parent, with its own New, Edit
//! and Delete; a related record's form is the form engine mounted under the
//! parent's path, its foreign key forced from the URL rather than trusted from
//! the client. Nothing binds before the parent exists: a create page names the
//! sections it will have, and "Save and continue editing" lands on them.

use std::collections::HashMap;
use std::sync::Arc;

use askama::Template;
use axum::extract::{Form, Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use laterite_auth::AuthenticatedUser;
use laterite_core::query::{build, text_cast};
use laterite_core::{t, Actor, Db, Text};
use sea_query::{Alias, Expr, Query as Sql};
use serde::{Deserialize, Serialize};

use crate::bulk::{self, BulkContext};
use crate::form::{self, FormConfig, PreparedForm, RelationSection};
use crate::list::{self, ListConfig, ListParams};
use crate::session::{FlashLevel, SessionHandle};
use crate::{not_found, render_error, AdminState, Shell};

/// One relation a resource declares: the rows of `entity` whose `foreign_key`
/// holds a parent record's id, listed and edited on the parent's edit page.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
#[non_exhaustive]
pub struct RelationConfig {
    /// The relation's name: the key in a descriptor, the path segment under
    /// the parent (`/products/7/variants`). Not `new`, `edit`, `delete`,
    /// `import`, `export` or `columns`, which the parent's own routes use.
    pub name: String,
    /// The section heading, plural (`Variants`). Defaults to the name,
    /// humanized.
    pub title: Text,
    /// One related record, singular (`Variant`), for the New button and the
    /// child form's title.
    pub label: Text,
    /// The related table.
    pub entity: String,
    /// The column on the related table holding the parent's id.
    pub foreign_key: String,
    /// The section's list over the related table.
    pub list: ListConfig,
    /// The related record's form. The foreign key is written by the framework;
    /// a field of that name here is kept hidden.
    pub form: FormConfig,
    /// A permission the operator must hold for the section and its pages,
    /// besides the parent's. `None` asks for the parent's alone.
    pub permission: Option<String>,
}

impl RelationConfig {
    /// A has-many relation: rows of `entity` whose `foreign_key` names the
    /// parent, listed by `list` and edited through `form`.
    pub fn has_many(
        name: impl Into<String>,
        label: impl Into<Text>,
        entity: impl Into<String>,
        foreign_key: impl Into<String>,
        list: ListConfig,
        form: FormConfig,
    ) -> Self {
        let name = name.into();
        Self {
            title: crate::descriptor::humanize(&name),
            name,
            label: label.into(),
            entity: entity.into(),
            foreign_key: foreign_key.into(),
            list,
            form,
            permission: None,
        }
    }

    /// The section heading.
    pub fn title(mut self, title: impl Into<Text>) -> Self {
        self.title = title.into();
        self
    }

    /// A permission the operator must also hold.
    pub fn permission(mut self, code: impl Into<String>) -> Self {
        self.permission = Some(code.into());
        self
    }
}

impl Default for RelationConfig {
    fn default() -> Self {
        Self::has_many("", "", "", "", ListConfig::default(), FormConfig::default())
    }
}

impl crate::keyed::Keyed for RelationConfig {
    fn key(&self) -> &str {
        &self.name
    }
    fn set_key(&mut self, key: String) {
        if self.title.source().is_empty() || self.title.source() == self.name {
            self.title = crate::descriptor::humanize(&key);
        }
        self.name = key;
    }
}

/// The names a relation may not take: the parent's own routes under `{id}`.
pub(crate) const RESERVED: [&str; 6] = ["new", "edit", "delete", "import", "export", "columns"];

/// A relation resolved at boot: its child form prepared, its delete context
/// built, ready to be scoped to a parent per request.
pub(crate) struct PreparedRelation {
    pub config: RelationConfig,
    /// The parent's path under the admin mount (`/admin/products`).
    pub parent_base: String,
    pub parent_entity: String,
    pub parent_id_field: String,
    form: Arc<PreparedForm>,
    bulk: Arc<BulkContext>,
}

impl PreparedRelation {
    pub(crate) fn prepare(
        parent: &crate::Resource,
        mut config: RelationConfig,
        field_types: &crate::field::FieldRegistry,
        column_types: &list::ColumnRegistry,
        permission_codes: &std::collections::HashSet<String>,
        persisters: &crate::persist::PersisterRegistry,
        listeners: &[laterite_core::ModelListenerReg],
    ) -> Result<Self, String> {
        if !crate::sql::valid_ident(&config.name) || RESERVED.contains(&config.name.as_str()) {
            return Err(format!(
                "relation `{}` has a name the parent's own routes use",
                config.name
            ));
        }
        if !crate::sql::valid_ident(&config.foreign_key) {
            return Err(format!(
                "relation `{}` names an invalid foreign key",
                config.name
            ));
        }
        config.list.entity = config.entity.clone();
        config.form.entity = config.entity.clone();
        config.form.id_field = config.list.id_field.clone();
        if config.form.title.source().is_empty() {
            config.form.title = config.label.clone();
        }
        // The foreign key is the framework's to write: a declared field of that
        // name stays in the form but never shows, and an undeclared one is added
        // hidden so the submission carries it.
        match config
            .form
            .fields
            .iter_mut()
            .find(|f| f.name == config.foreign_key)
        {
            Some(field) => field.hidden = true,
            None => {
                let mut field =
                    form::FormField::text(&config.foreign_key, config.foreign_key.clone());
                field.hidden = true;
                config.form.fields.push(field);
            }
        }
        list::check(&config.list, column_types, permission_codes)
            .map_err(|e| format!("relation `{}`: {e}", config.name))?;
        let prepared =
            PreparedForm::prepare(config.form.clone(), field_types, persisters, listeners)
                .map_err(|e| format!("relation `{}`: {e}", config.name))?;
        let persister = prepared.persister();
        let bulk = Arc::new(BulkContext {
            base_path: String::new(),
            entity: config.entity.clone(),
            persister,
            listeners: listeners
                .iter()
                .filter(|reg| reg.matches(&config.entity))
                .map(|reg| reg.listener.clone())
                .collect(),
        });
        Ok(Self {
            parent_base: parent.base_path.clone(),
            parent_entity: parent.list.entity.clone(),
            parent_id_field: parent.list.id_field.clone(),
            config,
            form: Arc::new(prepared),
            bulk,
        })
    }

    /// The section's path under a parent record.
    pub(crate) fn path(&self, parent_id: &str) -> String {
        format!("{}/{}/{}", self.parent_base, parent_id, self.config.name)
    }

    /// The parent's edit page, where a save returns to.
    fn parent_edit(&self, parent_id: &str) -> String {
        format!("{}/{}/edit", self.parent_base, parent_id)
    }

    /// The child form mounted under this parent.
    fn form_for(&self, parent_id: &str) -> Arc<PreparedForm> {
        let mut form = (*self.form).clone();
        form.config.base_path = self.path(parent_id);
        form.config.done_path = Some(self.parent_edit(parent_id));
        Arc::new(form)
    }

    /// The section's list scoped to this parent.
    fn list_for(&self, parent_id: &str) -> ListConfig {
        let mut list = self.config.list.clone();
        list.edit_base = Some(self.path(parent_id));
        list.creatable = true;
        list.scope = Some((self.config.foreign_key.clone(), parent_id.to_string()));
        list.remember = false;
        list
    }

    /// Whether the parent record exists; a child page under a parent that does
    /// not is a 404, never a write with a dangling key.
    async fn parent_exists(&self, db: &Db, parent_id: &str) -> Result<bool, sqlx::Error> {
        let (sql, values) = {
            let cast = text_cast(db.backend);
            build(
                db.backend,
                Sql::select()
                    .expr(Expr::col(Alias::new(&self.parent_id_field)).count())
                    .from(Alias::new(&self.parent_entity))
                    .and_where(
                        Expr::col(Alias::new(&self.parent_id_field))
                            .cast_as(Alias::new(cast))
                            .eq(parent_id.to_string()),
                    )
                    .to_owned(),
            )
        };
        let (n,): (i64,) = laterite_core::query::bind_values_as(sqlx::query_as(&sql), values)
            .fetch_one(&db.pool)
            .await?;
        Ok(n > 0)
    }

    /// The section as the parent's edit page shows it: page one, unsorted.
    pub(crate) async fn section(
        &self,
        state: &AdminState,
        parent_id: &str,
        user: &AuthenticatedUser,
        shell: &Shell,
    ) -> Option<RelationSection> {
        if let Some(code) = &self.config.permission {
            if !user.permissions.allows(code) {
                return None;
            }
        }
        let path = self.path(parent_id);
        let html = list::fragment(
            state,
            &self.list_for(parent_id),
            &path,
            ListParams::default(),
            &HashMap::new(),
            user,
            shell.clone(),
            &format!("lat-rel-{}", self.config.name),
            Some(format!("{path}/delete")),
        )
        .await
        .ok()?;
        Some(RelationSection {
            title: shell.tt(&self.config.title),
            new_href: Some(format!("{path}/new")),
            new_label: shell.tt(&t!("New {label}", label = shell.tt(&self.config.label))),
            html,
        })
    }
}

/// The handlers, each over one relation under one parent.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn index(
    State(state): State<AdminState>,
    Extension(shell): Extension<Shell>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(parent_id): Path<String>,
    Query(params): Query<ListParams>,
    Query(raw): Query<HashMap<String, String>>,
    headers: axum::http::HeaderMap,
    rel: Arc<PreparedRelation>,
) -> Response {
    match rel.parent_exists(&state.db, &parent_id).await {
        Ok(true) => {}
        Ok(false) => return not_found(),
        Err(_) => return render_error(),
    }
    let path = rel.path(&parent_id);
    let html = match list::fragment(
        &state,
        &rel.list_for(&parent_id),
        &path,
        params,
        &raw,
        &user,
        shell.clone(),
        &format!("lat-rel-{}", rel.config.name),
        Some(format!("{path}/delete")),
    )
    .await
    {
        Ok(html) => html,
        Err(()) => return render_error(),
    };
    if form::is_htmx(&headers) {
        return axum::response::Html(html.into_string()).into_response();
    }
    // Without scripting, a sort or pager link lands here: the section on a
    // page of its own, under the parent.
    let page = SectionPage {
        shell: &shell,
        back: rel.parent_edit(&parent_id),
        new_href: format!("{path}/new"),
        new_label: shell.tt(&t!("New {label}", label = shell.tt(&rel.config.label))),
        html,
    };
    match crate::html::Markup::from_template(&page) {
        Ok(body) => shell.page(&rel.config.title, body),
        Err(_) => render_error(),
    }
}

/// A section on a page of its own, for a request without scripting.
#[derive(Template)]
#[template(path = "relation_section.html")]
struct SectionPage<'a> {
    shell: &'a Shell,
    back: String,
    new_href: String,
    new_label: String,
    html: crate::html::Markup,
}

pub(crate) async fn new_form(
    State(state): State<AdminState>,
    Extension(shell): Extension<Shell>,
    Path(parent_id): Path<String>,
    rel: Arc<PreparedRelation>,
) -> Response {
    match rel.parent_exists(&state.db, &parent_id).await {
        Ok(true) => {}
        Ok(false) => return not_found(),
        Err(_) => return render_error(),
    }
    form::new_form(&state, &rel.form_for(&parent_id), shell, Vec::new())
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn create(
    State(state): State<AdminState>,
    Extension(shell): Extension<Shell>,
    Extension(user): Extension<AuthenticatedUser>,
    Extension(session): Extension<SessionHandle>,
    Path(parent_id): Path<String>,
    headers: axum::http::HeaderMap,
    Form(mut data): Form<HashMap<String, String>>,
    rel: Arc<PreparedRelation>,
) -> Response {
    match rel.parent_exists(&state.db, &parent_id).await {
        Ok(true) => {}
        Ok(false) => return not_found(),
        Err(_) => return render_error(),
    }
    // The parent is the URL's, whatever the submission said.
    data.insert(rel.config.foreign_key.clone(), parent_id.clone());
    form::create(
        &state,
        &rel.form_for(&parent_id),
        data,
        shell,
        &user,
        &session,
        &headers,
    )
    .await
}

pub(crate) async fn edit_form(
    State(state): State<AdminState>,
    Extension(shell): Extension<Shell>,
    Path((parent_id, child_id)): Path<(String, String)>,
    rel: Arc<PreparedRelation>,
) -> Response {
    match rel.belongs(&state.db, &parent_id, &child_id).await {
        Ok(true) => {}
        Ok(false) => return not_found(),
        Err(_) => return render_error(),
    }
    form::edit_form(
        &state,
        &rel.form_for(&parent_id),
        child_id,
        shell,
        Vec::new(),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn update(
    State(state): State<AdminState>,
    Extension(shell): Extension<Shell>,
    Extension(user): Extension<AuthenticatedUser>,
    Extension(session): Extension<SessionHandle>,
    Path((parent_id, child_id)): Path<(String, String)>,
    headers: axum::http::HeaderMap,
    Form(mut data): Form<HashMap<String, String>>,
    rel: Arc<PreparedRelation>,
) -> Response {
    match rel.belongs(&state.db, &parent_id, &child_id).await {
        Ok(true) => {}
        Ok(false) => return not_found(),
        Err(_) => return render_error(),
    }
    data.insert(rel.config.foreign_key.clone(), parent_id.clone());
    form::update(
        &state,
        &rel.form_for(&parent_id),
        child_id,
        data,
        shell,
        &user,
        &session,
        &headers,
    )
    .await
}

/// Removes the selected related records, then shows the section again.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn delete(
    State(state): State<AdminState>,
    Extension(shell): Extension<Shell>,
    Extension(user): Extension<AuthenticatedUser>,
    Extension(session): Extension<SessionHandle>,
    Path(parent_id): Path<String>,
    headers: axum::http::HeaderMap,
    Form(pairs): Form<Vec<(String, String)>>,
    rel: Arc<PreparedRelation>,
) -> Response {
    let ids: Vec<String> = pairs
        .iter()
        .filter(|(k, _)| k == "id")
        .map(|(_, v)| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .collect();
    let mut owned = Vec::new();
    for id in &ids {
        match rel.belongs(&state.db, &parent_id, id).await {
            Ok(true) => owned.push(id.clone()),
            Ok(false) => {}
            Err(_) => return render_error(),
        }
    }
    let actor = Actor::from(&user);
    let outcome = bulk::delete_selected(&state, &rel.bulk, &owned, &actor).await;
    if outcome.deleted > 0 {
        session.push_flash(
            FlashLevel::Success,
            laterite_core::tn!(
                "Deleted {n} record.",
                "Deleted {n} records.",
                n = outcome.deleted as i64
            ),
        );
    }
    for message in outcome.refused {
        session.push_flash(FlashLevel::Error, message);
    }
    if outcome.failed > 0 {
        session.push_flash(FlashLevel::Error, t!("Some records could not be deleted."));
    }
    // Back to the parent page whole, so the flash saying what happened shows.
    let _ = shell;
    form::saved_response(form::is_htmx(&headers), &rel.parent_edit(&parent_id))
}

impl PreparedRelation {
    /// Whether `child_id` is a related record of `parent_id`: a child page
    /// under the wrong parent is a 404, so an id cannot be walked across parents.
    async fn belongs(&self, db: &Db, parent_id: &str, child_id: &str) -> Result<bool, sqlx::Error> {
        let (sql, values) = {
            let cast = text_cast(db.backend);
            build(
                db.backend,
                Sql::select()
                    .expr(Expr::col(Alias::new(&self.config.list.id_field)).count())
                    .from(Alias::new(&self.config.entity))
                    .and_where(
                        Expr::col(Alias::new(&self.config.list.id_field))
                            .cast_as(Alias::new(cast))
                            .eq(child_id.to_string()),
                    )
                    .and_where(
                        Expr::col(Alias::new(&self.config.foreign_key))
                            .cast_as(Alias::new(cast))
                            .eq(parent_id.to_string()),
                    )
                    .to_owned(),
            )
        };
        let (n,): (i64,) = laterite_core::query::bind_values_as(sqlx::query_as(&sql), values)
            .fetch_one(&db.pool)
            .await?;
        Ok(n > 0)
    }
}
