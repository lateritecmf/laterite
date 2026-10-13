//! Related records on a form: a resource declares which other entity's rows
//! belong to each of its records, and the edit page carries a section for them.
//!
//! A section is the list engine scoped to the parent, with its own New, Edit
//! and Delete; a related record's form is the form engine mounted under the
//! parent's path, its foreign key forced from the URL rather than trusted from
//! the client. A many-to-many relation lists the records a pivot table joins
//! to the parent: Attach picks one and writes the pivot row, Edit changes
//! what the pivot row carries, Remove deletes the pivot row and leaves the
//! record. Nothing binds before the parent exists: a create page names the
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

/// One relation a resource declares, listed and edited on the parent's edit
/// page: rows of `entity` whose `foreign_key` holds the parent's id (has-many),
/// or rows of `entity` a `pivot` table joins to the parent (many-to-many).
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
    /// One related record, singular (`Variant`), for the New or Attach button
    /// and the child form's title.
    pub label: Text,
    /// The related table.
    pub entity: String,
    /// Has-many: the column on the related table holding the parent's id.
    pub foreign_key: Option<String>,
    /// Many-to-many: the table joining the parent to the related table.
    pub pivot: Option<PivotConfig>,
    /// The section's list over the related table.
    pub list: ListConfig,
    /// Has-many: the related record's form; the foreign key is written by the
    /// framework, and a field of that name here is kept hidden. Many-to-many:
    /// the fields kept on the pivot row, asked for on Attach and editable from
    /// the section; none means a membership carries nothing of its own.
    pub form: FormConfig,
    /// A permission the operator must hold for the section and its pages,
    /// besides the parent's. `None` asks for the parent's alone.
    pub permission: Option<String>,
}

/// The table joining a many-to-many relation's two sides, one row per
/// membership.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
#[non_exhaustive]
pub struct PivotConfig {
    /// The pivot table.
    pub table: String,
    /// The column holding the parent's id.
    pub parent_key: String,
    /// The column holding the related record's id.
    pub related_key: String,
    /// The picker source offering records to attach: a registered
    /// [`PickerSourceReg`](crate::picker::PickerSourceReg) name.
    pub source: String,
    /// The pivot row's id column. Default `id`.
    pub id_field: String,
}

impl PivotConfig {
    /// A pivot table, with records to attach chosen from `source`.
    pub fn new(
        table: impl Into<String>,
        parent_key: impl Into<String>,
        related_key: impl Into<String>,
        source: impl Into<String>,
    ) -> Self {
        Self {
            table: table.into(),
            parent_key: parent_key.into(),
            related_key: related_key.into(),
            source: source.into(),
            id_field: "id".to_string(),
        }
    }

    /// The pivot row's id column.
    pub fn id_field(mut self, id_field: impl Into<String>) -> Self {
        self.id_field = id_field.into();
        self
    }
}

impl Default for PivotConfig {
    fn default() -> Self {
        Self::new("", "", "", "")
    }
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
            foreign_key: Some(foreign_key.into()),
            pivot: None,
            list,
            form,
            permission: None,
        }
    }

    /// A many-to-many relation: rows of `entity` that `pivot` joins to the
    /// parent, listed by `list`. Fields kept on the pivot row go on [`form`].
    ///
    /// [`form`]: RelationConfig::form
    pub fn belongs_to_many(
        name: impl Into<String>,
        label: impl Into<Text>,
        entity: impl Into<String>,
        pivot: PivotConfig,
        list: ListConfig,
    ) -> Self {
        let name = name.into();
        Self {
            title: crate::descriptor::humanize(&name),
            name,
            label: label.into(),
            entity: entity.into(),
            foreign_key: None,
            pivot: Some(pivot),
            list,
            form: FormConfig::default(),
            permission: None,
        }
    }

    /// The fields kept on a many-to-many relation's pivot row.
    pub fn form(mut self, form: FormConfig) -> Self {
        self.form = form;
        self
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
        Self {
            name: String::new(),
            title: Text::from(""),
            label: Text::from(""),
            entity: String::new(),
            foreign_key: None,
            pivot: None,
            list: ListConfig::default(),
            form: FormConfig::default(),
            permission: None,
        }
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

/// How a relation's rows are tied to the parent.
enum Kind {
    HasMany {
        foreign_key: String,
    },
    ManyToMany {
        pivot: PivotConfig,
        /// The Attach form: the pivot fields plus a picker for the record.
        attach: Arc<PreparedForm>,
    },
}

/// A relation resolved at boot: its forms prepared, its delete context built,
/// ready to be scoped to a parent per request.
pub(crate) struct PreparedRelation {
    pub config: RelationConfig,
    /// The parent's path under the admin mount (`/admin/products`).
    pub parent_base: String,
    pub parent_entity: String,
    pub parent_id_field: String,
    kind: Kind,
    /// Has-many: the child form. Many-to-many: the pivot row's form.
    form: Arc<PreparedForm>,
    bulk: Arc<BulkContext>,
}

/// Keeps a key field of this name out of sight: a declared one is hidden, an
/// undeclared one is added hidden so the submission carries it. Either way it
/// is stored as an id (a number), which a bigint column takes on every
/// backend; a declared text field is retyped, any other type is kept.
fn hide(form: &mut FormConfig, name: &str) {
    match form.fields.iter_mut().find(|f| f.name == name) {
        Some(field) => {
            field.hidden = true;
            if field.field_type == "text" {
                field.field_type = "id".to_string();
            }
        }
        None => {
            let mut field = form::FormField::of(name, name.to_string(), "id");
            field.hidden = true;
            form.fields.push(field);
        }
    }
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
        let name = config.name.clone();
        let fail = |message: String| format!("relation `{name}`: {message}");
        if !crate::sql::valid_ident(&config.name) || RESERVED.contains(&config.name.as_str()) {
            return Err(fail("the name is one the parent's own routes use".into()));
        }
        config.list.entity = config.entity.clone();
        list::check(&config.list, column_types, permission_codes).map_err(fail)?;
        if config.form.title.source().is_empty() {
            config.form.title = config.label.clone();
        }
        let prepare = |form: FormConfig| {
            PreparedForm::prepare(form, field_types, persisters, listeners).map_err(fail)
        };
        let (kind, form) = match (config.foreign_key.clone(), config.pivot.clone()) {
            (Some(foreign_key), None) => {
                if !crate::sql::valid_ident(&foreign_key) {
                    return Err(fail("the foreign key is not a valid identifier".into()));
                }
                config.form.entity = config.entity.clone();
                config.form.id_field = config.list.id_field.clone();
                // The foreign key is the framework's to write.
                hide(&mut config.form, &foreign_key);
                (Kind::HasMany { foreign_key }, prepare(config.form.clone())?)
            }
            (None, Some(pivot)) => {
                if ![
                    &pivot.table,
                    &pivot.parent_key,
                    &pivot.related_key,
                    &pivot.id_field,
                ]
                .iter()
                .all(|ident| crate::sql::valid_ident(ident))
                {
                    return Err(fail("the pivot names an invalid identifier".into()));
                }
                if pivot.source.is_empty() {
                    return Err(fail("the pivot names no picker source".into()));
                }
                config.form.entity = pivot.table.clone();
                config.form.id_field = pivot.id_field.clone();
                hide(&mut config.form, &pivot.parent_key);
                // Attach asks which record, through the picker; afterwards the
                // membership is fixed and the form hides it.
                let mut attach = config.form.clone();
                if !attach.fields.iter().any(|f| f.name == pivot.related_key) {
                    attach.fields.insert(
                        0,
                        form::FormField::reference(
                            &pivot.related_key,
                            config.label.clone(),
                            &pivot.source,
                        )
                        .required(),
                    );
                }
                hide(&mut config.form, &pivot.related_key);
                let attach = Arc::new(prepare(attach)?);
                (
                    Kind::ManyToMany { pivot, attach },
                    prepare(config.form.clone())?,
                )
            }
            _ => return Err(fail("declare exactly one of foreign_key and pivot".into())),
        };
        let persister = form.persister();
        let entity = form.config.entity.clone();
        let bulk = Arc::new(BulkContext {
            base_path: String::new(),
            listeners: listeners
                .iter()
                .filter(|reg| reg.matches(&entity))
                .map(|reg| reg.listener.clone())
                .collect(),
            entity,
            persister,
        });
        Ok(Self {
            parent_base: parent.base_path.clone(),
            parent_entity: parent.list.entity.clone(),
            parent_id_field: parent.list.id_field.clone(),
            config,
            kind,
            form: Arc::new(form),
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

    /// A form mounted under this parent.
    fn rebase(&self, form: &PreparedForm, parent_id: &str) -> Arc<PreparedForm> {
        let mut form = form.clone();
        form.config.base_path = self.path(parent_id);
        form.config.done_path = Some(self.parent_edit(parent_id));
        Arc::new(form)
    }

    /// The form behind New (has-many) or Attach (many-to-many).
    fn new_form_for(&self, parent_id: &str) -> Arc<PreparedForm> {
        match &self.kind {
            Kind::HasMany { .. } => self.rebase(&self.form, parent_id),
            Kind::ManyToMany { attach, .. } => self.rebase(attach, parent_id),
        }
    }

    /// The form behind a row's Edit.
    fn form_for(&self, parent_id: &str) -> Arc<PreparedForm> {
        self.rebase(&self.form, parent_id)
    }

    /// The section's list scoped to this parent.
    fn list_for(&self, parent_id: &str) -> ListConfig {
        let mut list = self.config.list.clone();
        list.remember = false;
        match &self.kind {
            Kind::HasMany { foreign_key } => {
                list.edit_base = Some(self.path(parent_id));
                list.creatable = true;
                // A section is one parent's rows, never a tree of its own.
                list.tree = None;
                list.scope = Some(list::Scope::Column {
                    column: foreign_key.clone(),
                    value: Some(parent_id.to_string()),
                });
            }
            Kind::ManyToMany { pivot, .. } => {
                // The order would be the pivot's to hold, and the list is not
                // over the pivot.
                list.reorder = None;
                // A membership with nothing of its own has nothing to edit.
                list.edit_base =
                    (!self.config.form.fields.is_empty()).then(|| self.path(parent_id));
                list.creatable = false;
                list.scope = Some(list::Scope::Pivot {
                    table: pivot.table.clone(),
                    parent_key: pivot.parent_key.clone(),
                    related_key: pivot.related_key.clone(),
                    id_field: pivot.id_field.clone(),
                    value: parent_id.to_string(),
                });
            }
        }
        list
    }

    /// Whether the section's rows keep an order of their own: a has-many
    /// whose list declares `reorder`.
    pub(crate) fn reorderable(&self) -> bool {
        matches!(self.kind, Kind::HasMany { .. }) && self.config.list.reorder.is_some()
    }

    /// The per-row action: Delete for a record, Remove for a membership.
    fn row_action(&self, path: &str, shell: &Shell) -> list::RowAction {
        let (label, confirm, icon) = match &self.kind {
            Kind::HasMany { .. } => (t!("Delete"), t!("Delete this record?"), "trash-2"),
            Kind::ManyToMany { .. } => (t!("Remove"), t!("Remove this record?"), "unlink"),
        };
        list::RowAction {
            path: format!("{path}/delete"),
            label: shell.tt(&label),
            confirm: shell.tt(&confirm),
            icon,
        }
    }

    /// The New or Attach button's label.
    fn new_label(&self, shell: &Shell) -> String {
        let label = shell.tt(&self.config.label);
        match &self.kind {
            Kind::HasMany { .. } => shell.tt(&t!("New {label}", label = label)),
            Kind::ManyToMany { .. } => shell.tt(&t!("Attach {label}", label = label)),
        }
    }

    /// Whether one row of `table` has every column equal to its value.
    async fn exists(
        &self,
        db: &Db,
        table: &str,
        pairs: &[(&str, &str)],
    ) -> Result<bool, sqlx::Error> {
        let (sql, values) = {
            let cast = text_cast(db.backend);
            let mut select = Sql::select();
            select.expr(Expr::val(1).count()).from(Alias::new(table));
            for (column, value) in pairs {
                select.and_where(
                    Expr::col(Alias::new(*column))
                        .cast_as(Alias::new(cast))
                        .eq(value.to_string()),
                );
            }
            build(db.backend, select)
        };
        let (n,): (i64,) = laterite_core::query::bind_values_as(sqlx::query_as(&sql), values)
            .fetch_one(&db.pool)
            .await?;
        Ok(n > 0)
    }

    /// Whether the parent record exists; a child page under a parent that does
    /// not is a 404, never a write with a dangling key.
    async fn parent_exists(&self, db: &Db, parent_id: &str) -> Result<bool, sqlx::Error> {
        self.exists(
            db,
            &self.parent_entity,
            &[(&self.parent_id_field, parent_id)],
        )
        .await
    }

    /// Whether `child_id` is this parent's: a has-many's related record, or a
    /// many-to-many's pivot row. A child page under the wrong parent is a 404,
    /// so an id cannot be walked across parents.
    async fn owned(&self, db: &Db, parent_id: &str, child_id: &str) -> Result<bool, sqlx::Error> {
        match &self.kind {
            Kind::HasMany { foreign_key } => {
                self.exists(
                    db,
                    &self.config.entity,
                    &[
                        (&self.config.list.id_field, child_id),
                        (foreign_key, parent_id),
                    ],
                )
                .await
            }
            Kind::ManyToMany { pivot, .. } => {
                self.exists(
                    db,
                    &pivot.table,
                    &[(&pivot.id_field, child_id), (&pivot.parent_key, parent_id)],
                )
                .await
            }
        }
    }

    /// The related record a pivot row names, when the row is this parent's.
    async fn pivot_related(
        &self,
        db: &Db,
        pivot: &PivotConfig,
        parent_id: &str,
        child_id: &str,
    ) -> Result<Option<String>, sqlx::Error> {
        let (sql, values) = {
            let cast = text_cast(db.backend);
            build(
                db.backend,
                Sql::select()
                    .expr_as(
                        Expr::col(Alias::new(&pivot.related_key)).cast_as(Alias::new(cast)),
                        Alias::new("related"),
                    )
                    .from(Alias::new(&pivot.table))
                    .and_where(
                        Expr::col(Alias::new(&pivot.id_field))
                            .cast_as(Alias::new(cast))
                            .eq(child_id.to_string()),
                    )
                    .and_where(
                        Expr::col(Alias::new(&pivot.parent_key))
                            .cast_as(Alias::new(cast))
                            .eq(parent_id.to_string()),
                    )
                    .to_owned(),
            )
        };
        let row: Option<(String,)> =
            laterite_core::query::bind_values_as(sqlx::query_as(&sql), values)
                .fetch_optional(&db.pool)
                .await?;
        Ok(row.map(|(related,)| related))
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
            Some(self.row_action(&path, shell)),
        )
        .await
        .ok()?;
        Some(RelationSection {
            title: shell.tt(&self.config.title),
            new_href: Some(format!("{path}/new")),
            new_label: self.new_label(shell),
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
        Some(rel.row_action(&path, &shell)),
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
        new_label: rel.new_label(&shell),
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
    form::new_form(&state, &rel.new_form_for(&parent_id), shell, Vec::new())
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
    match &rel.kind {
        Kind::HasMany { foreign_key } => {
            data.insert(foreign_key.clone(), parent_id.clone());
        }
        Kind::ManyToMany { pivot, .. } => {
            data.insert(pivot.parent_key.clone(), parent_id.clone());
            // A blank is the form's to refuse, as a required field.
            let related = data
                .get(&pivot.related_key)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty());
            if let Some(related) = related {
                let present = rel
                    .exists(
                        &state.db,
                        &rel.config.entity,
                        &[(&rel.config.list.id_field, &related)],
                    )
                    .await;
                let attached = rel
                    .exists(
                        &state.db,
                        &pivot.table,
                        &[
                            (&pivot.parent_key, &parent_id),
                            (&pivot.related_key, &related),
                        ],
                    )
                    .await;
                let refusal = match (present, attached) {
                    (Ok(false), _) => Some(t!("There is no such record to attach.")),
                    (_, Ok(true)) => Some(t!("That record is already attached.")),
                    (Err(_), _) | (_, Err(_)) => return render_error(),
                    _ => None,
                };
                if let Some(message) = refusal {
                    session.push_flash(FlashLevel::Error, message);
                    let back = format!("{}/new", rel.path(&parent_id));
                    return form::saved_response(form::is_htmx(&headers), &back);
                }
            }
        }
    }
    form::create(
        &state,
        &rel.new_form_for(&parent_id),
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
    match rel.owned(&state.db, &parent_id, &child_id).await {
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
    // The parent is the URL's and the membership is the row's, whatever the
    // submission said.
    match &rel.kind {
        Kind::HasMany { foreign_key } => {
            match rel.owned(&state.db, &parent_id, &child_id).await {
                Ok(true) => {}
                Ok(false) => return not_found(),
                Err(_) => return render_error(),
            }
            data.insert(foreign_key.clone(), parent_id.clone());
        }
        Kind::ManyToMany { pivot, .. } => {
            match rel
                .pivot_related(&state.db, pivot, &parent_id, &child_id)
                .await
            {
                Ok(Some(related)) => {
                    data.insert(pivot.related_key.clone(), related);
                }
                Ok(None) => return not_found(),
                Err(_) => return render_error(),
            }
            data.insert(pivot.parent_key.clone(), parent_id.clone());
        }
    }
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

/// Moves one related record within its parent's section.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn reorder(
    State(state): State<AdminState>,
    Extension(shell): Extension<Shell>,
    Extension(user): Extension<AuthenticatedUser>,
    Extension(session): Extension<SessionHandle>,
    Path(parent_id): Path<String>,
    headers: axum::http::HeaderMap,
    Form(pairs): Form<Vec<(String, String)>>,
    rel: Arc<PreparedRelation>,
) -> Response {
    match rel.parent_exists(&state.db, &parent_id).await {
        Ok(true) => {}
        Ok(false) => return not_found(),
        Err(_) => return render_error(),
    }
    let path = rel.path(&parent_id);
    let row_action = Some(rel.row_action(&path, &shell));
    list::reorder_handler(
        &state,
        &rel.list_for(&parent_id),
        &path,
        &format!("lat-rel-{}", rel.config.name),
        row_action,
        &user,
        &session,
        shell,
        &headers,
        &pairs,
    )
    .await
}

/// Removes the selected rows (a has-many's records, a many-to-many's
/// memberships), then shows the parent page again.
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
        match rel.owned(&state.db, &parent_id, id).await {
            Ok(true) => owned.push(id.clone()),
            Ok(false) => {}
            Err(_) => return render_error(),
        }
    }
    let actor = Actor::from(&user);
    let outcome = bulk::delete_selected(&state, &rel.bulk, &owned, &actor).await;
    if outcome.deleted > 0 {
        let n = outcome.deleted as i64;
        let message = match &rel.kind {
            Kind::HasMany { .. } => {
                laterite_core::tn!("Deleted {n} record.", "Deleted {n} records.", n = n)
            }
            Kind::ManyToMany { .. } => {
                laterite_core::tn!("Removed {n} record.", "Removed {n} records.", n = n)
            }
        };
        session.push_flash(FlashLevel::Success, message);
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
