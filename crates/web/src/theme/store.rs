//! Queries over the theme tables.
//!
//! A write that an operator makes from the admin is audited there; this layer
//! keeps the revision that lets a template save be undone, and refuses what the
//! rows forbid: a locked theme, a parent cycle, deleting the active theme.

use super::schema::{ThemeTemplateRevisions, ThemeTemplates, Themes};
use super::{valid_code, valid_path, Kind, Revision, Template, Theme, ThemeConfig, ThemeError};
use laterite_core::query::{bind_values, build, insert_returning_id};
use laterite_core::{AnyRowExt, Db};
use sea_query::{Expr, Order, Query};
use sqlx::any::AnyRow;

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn theme_from_row(row: &AnyRow) -> Result<Theme, ThemeError> {
    let config_source = row.get_text("config")?;
    let settings = row
        .get_text_opt("settings")?
        .and_then(|text| serde_json::from_str(&text).ok())
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    Ok(Theme {
        id: row.get_int("id")?,
        code: row.get_text("code")?,
        parent: row.get_text_opt("parent")?,
        locked: row.get_bool("locked")?,
        config: ThemeConfig::parse(&config_source)?,
        config_source,
        settings,
        created_at: row.get_text("created_at")?,
        updated_at: row.get_text("updated_at")?,
    })
}

fn template_from_row(row: &AnyRow) -> Result<Template, ThemeError> {
    let settings = row.get_text("settings")?;
    Ok(Template {
        id: row.get_int("id")?,
        theme_id: row.get_int("theme_id")?,
        kind: row.get_text("kind")?.parse()?,
        path: row.get_text("path")?,
        settings: serde_json::from_str(&settings).unwrap_or(serde_json::Value::Null),
        markup: row.get_text("markup")?,
        created_at: row.get_text("created_at")?,
        updated_at: row.get_text("updated_at")?,
    })
}

fn revision_from_row(row: &AnyRow) -> Result<Revision, ThemeError> {
    let settings = row.get_text("settings")?;
    Ok(Revision {
        id: row.get_int("id")?,
        template_id: row.get_int("template_id")?,
        settings: serde_json::from_str(&settings).unwrap_or(serde_json::Value::Null),
        markup: row.get_text("markup")?,
        saved_at: row.get_text("saved_at")?,
        saved_by: row.get_int_opt("saved_by")?,
    })
}

const THEME_COLUMNS: [Themes; 8] = [
    Themes::Id,
    Themes::Code,
    Themes::Parent,
    Themes::Locked,
    Themes::Config,
    Themes::Settings,
    Themes::CreatedAt,
    Themes::UpdatedAt,
];

const TEMPLATE_COLUMNS: [ThemeTemplates; 8] = [
    ThemeTemplates::Id,
    ThemeTemplates::ThemeId,
    ThemeTemplates::Kind,
    ThemeTemplates::Path,
    ThemeTemplates::Settings,
    ThemeTemplates::Markup,
    ThemeTemplates::CreatedAt,
    ThemeTemplates::UpdatedAt,
];

/// Every theme, by code.
pub async fn list_themes(db: &Db) -> Result<Vec<Theme>, ThemeError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns(THEME_COLUMNS)
            .from(Themes::Table)
            .order_by(Themes::Code, Order::Asc)
            .to_owned(),
    );
    let rows = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?;
    rows.iter().map(theme_from_row).collect()
}

/// The theme with this code, if any.
pub async fn find_theme(db: &Db, code: &str) -> Result<Option<Theme>, ThemeError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns(THEME_COLUMNS)
            .from(Themes::Table)
            .and_where(Expr::col(Themes::Code).eq(code))
            .to_owned(),
    );
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&db.pool)
        .await?;
    row.as_ref().map(theme_from_row).transpose()
}

async fn require_theme(db: &Db, code: &str) -> Result<Theme, ThemeError> {
    find_theme(db, code)
        .await?
        .ok_or_else(|| ThemeError::NotFound(code.to_string()))
}

/// Refuses a parent that does not exist or that leads back to `code`.
async fn check_parent(db: &Db, code: &str, parent: Option<&str>) -> Result<(), ThemeError> {
    let Some(parent) = parent else {
        return Ok(());
    };
    let mut next = Some(parent.to_string());
    let mut hops = 0;
    while let Some(current) = next {
        if current == code {
            return Err(ThemeError::Parent(format!(
                "the parent {parent:?} leads back to {code:?}"
            )));
        }
        hops += 1;
        if hops > 32 {
            return Err(ThemeError::Parent(format!(
                "the parent chain from {parent:?} is too long"
            )));
        }
        let theme = find_theme(db, &current)
            .await?
            .ok_or_else(|| ThemeError::Parent(format!("no parent theme {current:?}")))?;
        next = theme.parent;
    }
    Ok(())
}

/// Creates a theme from its code and YAML config.
pub async fn create_theme(db: &Db, code: &str, config_yaml: &str) -> Result<Theme, ThemeError> {
    if !valid_code(code) {
        return Err(ThemeError::Code(code.to_string()));
    }
    let config = ThemeConfig::parse(config_yaml)?;
    if find_theme(db, code).await?.is_some() {
        return Err(ThemeError::Exists(code.to_string()));
    }
    check_parent(db, code, config.parent.as_deref()).await?;
    let now = now();
    let insert = Query::insert()
        .into_table(Themes::Table)
        .columns([
            Themes::Code,
            Themes::Parent,
            Themes::Locked,
            Themes::Config,
            Themes::CreatedAt,
            Themes::UpdatedAt,
        ])
        .values_panic([
            code.into(),
            config.parent.clone().into(),
            false.into(),
            config_yaml.into(),
            now.clone().into(),
            now.into(),
        ])
        .to_owned();
    insert_returning_id(db, insert, Themes::Id).await?;
    require_theme(db, code).await
}

/// Replaces a theme's YAML config; the parent column follows it.
pub async fn update_theme_config(
    db: &Db,
    code: &str,
    config_yaml: &str,
) -> Result<Theme, ThemeError> {
    let theme = require_theme(db, code).await?;
    if theme.locked {
        return Err(ThemeError::Locked(code.to_string()));
    }
    let config = ThemeConfig::parse(config_yaml)?;
    check_parent(db, code, config.parent.as_deref()).await?;
    let (sql, values) = build(
        db.backend,
        Query::update()
            .table(Themes::Table)
            .value(Themes::Config, config_yaml)
            .value(Themes::Parent, config.parent.clone())
            .value(Themes::UpdatedAt, now())
            .and_where(Expr::col(Themes::Id).eq(theme.id))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    require_theme(db, code).await
}

/// Replaces a theme's settings values. `settings` must be a JSON object.
pub async fn save_settings(
    db: &Db,
    code: &str,
    settings: &serde_json::Value,
) -> Result<(), ThemeError> {
    if !settings.is_object() {
        return Err(ThemeError::Config(
            "the settings must be a JSON object".into(),
        ));
    }
    let theme = require_theme(db, code).await?;
    if theme.locked {
        return Err(ThemeError::Locked(code.to_string()));
    }
    let (sql, values) = build(
        db.backend,
        Query::update()
            .table(Themes::Table)
            .value(Themes::Settings, settings.to_string())
            .value(Themes::UpdatedAt, now())
            .and_where(Expr::col(Themes::Id).eq(theme.id))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

/// The settings form in force for a chain (nearest first): the first theme
/// that declares one, as the config falls through to the parent.
pub fn form_fields(chain: &[Theme]) -> Vec<laterite_admin::form::FormField> {
    chain
        .iter()
        .find_map(|t| t.config.form.as_ref())
        .map(|f| f.fields.clone())
        .unwrap_or_default()
}

/// Locks or unlocks a theme.
pub async fn set_locked(db: &Db, code: &str, locked: bool) -> Result<(), ThemeError> {
    let theme = require_theme(db, code).await?;
    let (sql, values) = build(
        db.backend,
        Query::update()
            .table(Themes::Table)
            .value(Themes::Locked, locked)
            .value(Themes::UpdatedAt, now())
            .and_where(Expr::col(Themes::Id).eq(theme.id))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

/// The codes of the themes whose parent is `code`.
pub async fn children(db: &Db, code: &str) -> Result<Vec<String>, ThemeError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .column(Themes::Code)
            .from(Themes::Table)
            .and_where(Expr::col(Themes::Parent).eq(code))
            .order_by(Themes::Code, Order::Asc)
            .to_owned(),
    );
    let rows = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?;
    rows.iter().map(|r| Ok(r.get_text("code")?)).collect()
}

/// Deletes a theme and its templates. Refuses the active theme, a locked
/// theme, and a theme other themes name as their parent.
pub async fn delete_theme(db: &Db, code: &str) -> Result<(), ThemeError> {
    let theme = require_theme(db, code).await?;
    if theme.locked {
        return Err(ThemeError::Locked(code.to_string()));
    }
    let settings: super::ThemeSettings = laterite_admin::settings::load(db).await?;
    if settings.active.as_deref() == Some(code) {
        return Err(ThemeError::Active(code.to_string()));
    }
    let dependants = children(db, code).await?;
    if !dependants.is_empty() {
        return Err(ThemeError::Parent(format!(
            "{code:?} is the parent of {}",
            dependants.join(", ")
        )));
    }
    let (sql, values) = build(
        db.backend,
        Query::delete()
            .from_table(Themes::Table)
            .and_where(Expr::col(Themes::Id).eq(theme.id))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}

/// Copies a theme, templates included, under a new code. The copy is unlocked
/// and keeps the original's parent.
pub async fn duplicate_theme(db: &Db, code: &str, new_code: &str) -> Result<Theme, ThemeError> {
    let source = require_theme(db, code).await?;
    let copy = create_theme(db, new_code, &source.config_source).await?;
    for t in list_templates(db, source.id, None).await? {
        save_template(
            db,
            copy.id,
            t.kind,
            &t.path,
            Some(t.settings),
            &t.markup,
            None,
        )
        .await?;
    }
    Ok(copy)
}

/// Creates an empty theme whose parent is `code`, named `name`.
pub async fn create_child_theme(
    db: &Db,
    code: &str,
    new_code: &str,
    name: &str,
) -> Result<Theme, ThemeError> {
    let parent = require_theme(db, code).await?;
    let config = ThemeConfig {
        name: name.to_string(),
        author: parent.config.author.clone(),
        homepage: parent.config.homepage.clone(),
        description: String::new(),
        preview: None,
        parent: Some(code.to_string()),
        require: parent.config.require.clone(),
        form: None,
    };
    create_theme(db, new_code, &config.to_yaml()?).await
}

/// A theme's own templates, by kind and path, or those of one kind.
pub async fn list_templates(
    db: &Db,
    theme_id: i64,
    kind: Option<Kind>,
) -> Result<Vec<Template>, ThemeError> {
    // Built and rendered in one block: a statement builder is not `Send`, and
    // one left in scope across the await makes the whole future not `Send`.
    let (sql, values) = {
        let mut query = Query::select()
            .columns(TEMPLATE_COLUMNS)
            .from(ThemeTemplates::Table)
            .and_where(Expr::col(ThemeTemplates::ThemeId).eq(theme_id))
            .order_by(ThemeTemplates::Kind, Order::Asc)
            .order_by(ThemeTemplates::Path, Order::Asc)
            .to_owned();
        if let Some(kind) = kind {
            query.and_where(Expr::col(ThemeTemplates::Kind).eq(kind.as_str()));
        }
        build(db.backend, query)
    };
    let rows = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?;
    rows.iter().map(template_from_row).collect()
}

/// A theme's own template at `path`, if any.
pub async fn find_template(
    db: &Db,
    theme_id: i64,
    kind: Kind,
    path: &str,
) -> Result<Option<Template>, ThemeError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns(TEMPLATE_COLUMNS)
            .from(ThemeTemplates::Table)
            .and_where(Expr::col(ThemeTemplates::ThemeId).eq(theme_id))
            .and_where(Expr::col(ThemeTemplates::Kind).eq(kind.as_str()))
            .and_where(Expr::col(ThemeTemplates::Path).eq(path))
            .to_owned(),
    );
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&db.pool)
        .await?;
    row.as_ref().map(template_from_row).transpose()
}

/// A template by id.
pub async fn template(db: &Db, id: i64) -> Result<Option<Template>, ThemeError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns(TEMPLATE_COLUMNS)
            .from(ThemeTemplates::Table)
            .and_where(Expr::col(ThemeTemplates::Id).eq(id))
            .to_owned(),
    );
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&db.pool)
        .await?;
    row.as_ref().map(template_from_row).transpose()
}

/// The template at `path` for the theme `code`: its own row, else the nearest
/// ancestor's. The theme it came from is returned with it.
pub async fn resolve_template(
    db: &Db,
    code: &str,
    kind: Kind,
    path: &str,
) -> Result<Option<(Theme, Template)>, ThemeError> {
    let mut theme = require_theme(db, code).await?;
    let mut hops = 0;
    loop {
        if let Some(t) = find_template(db, theme.id, kind, path).await? {
            return Ok(Some((theme, t)));
        }
        let Some(parent) = theme.parent.clone() else {
            return Ok(None);
        };
        hops += 1;
        if hops > 32 {
            return Ok(None);
        }
        theme = match find_theme(db, &parent).await? {
            Some(p) => p,
            None => return Ok(None),
        };
    }
}

/// A theme and its ancestors, nearest first: the theme, its parent, the
/// parent's parent. A missing ancestor ends the chain; a cycle is cut at 32.
pub async fn chain(db: &Db, code: &str) -> Result<Vec<Theme>, ThemeError> {
    let mut out = vec![require_theme(db, code).await?];
    while let Some(parent) = out.last().and_then(|t| t.parent.clone()) {
        if out.len() > 32 {
            break;
        }
        match find_theme(db, &parent).await? {
            Some(p) => out.push(p),
            None => break,
        }
    }
    Ok(out)
}

async fn theme_by_id(db: &Db, id: i64) -> Result<Theme, ThemeError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns(THEME_COLUMNS)
            .from(Themes::Table)
            .and_where(Expr::col(Themes::Id).eq(id))
            .to_owned(),
    );
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&db.pool)
        .await?;
    match row {
        Some(r) => theme_from_row(&r),
        None => Err(ThemeError::NotFound(id.to_string())),
    }
}

fn touch_theme(
    db: &Db,
    id: i64,
) -> impl std::future::Future<Output = Result<(), sqlx::Error>> + Send + '_ {
    let (sql, values) = build(
        db.backend,
        Query::update()
            .table(Themes::Table)
            .value(Themes::UpdatedAt, now())
            .and_where(Expr::col(Themes::Id).eq(id))
            .to_owned(),
    );
    async move {
        bind_values(sqlx::query(&sql), values)
            .execute(&db.pool)
            .await?;
        Ok(())
    }
}

/// Creates or replaces the template at `path`. `settings` of `None` keeps the
/// current bag (an empty object for a new template). A replaced template's
/// previous content is kept as a revision attributed to `saved_by`. Refused for
/// a locked theme.
pub async fn save_template(
    db: &Db,
    theme_id: i64,
    kind: Kind,
    path: &str,
    settings: Option<serde_json::Value>,
    markup: &str,
    saved_by: Option<i64>,
) -> Result<Template, ThemeError> {
    if !valid_path(path) {
        return Err(ThemeError::Path(path.to_string()));
    }
    let theme = theme_by_id(db, theme_id).await?;
    if theme.locked {
        return Err(ThemeError::Locked(theme.code));
    }
    let now = now();
    let id = match find_template(db, theme_id, kind, path).await? {
        Some(current) => {
            record_revision(db, &current, &now, saved_by).await?;
            let settings = settings.unwrap_or(current.settings);
            let (sql, values) = build(
                db.backend,
                Query::update()
                    .table(ThemeTemplates::Table)
                    .value(ThemeTemplates::Settings, settings.to_string())
                    .value(ThemeTemplates::Markup, markup)
                    .value(ThemeTemplates::UpdatedAt, now.clone())
                    .and_where(Expr::col(ThemeTemplates::Id).eq(current.id))
                    .to_owned(),
            );
            bind_values(sqlx::query(&sql), values)
                .execute(&db.pool)
                .await?;
            current.id
        }
        None => {
            let settings = settings.unwrap_or_else(|| serde_json::json!({}));
            let insert = Query::insert()
                .into_table(ThemeTemplates::Table)
                .columns([
                    ThemeTemplates::ThemeId,
                    ThemeTemplates::Kind,
                    ThemeTemplates::Path,
                    ThemeTemplates::Settings,
                    ThemeTemplates::Markup,
                    ThemeTemplates::CreatedAt,
                    ThemeTemplates::UpdatedAt,
                ])
                .values_panic([
                    theme_id.into(),
                    kind.as_str().into(),
                    path.into(),
                    settings.to_string().into(),
                    markup.into(),
                    now.clone().into(),
                    now.clone().into(),
                ])
                .to_owned();
            insert_returning_id(db, insert, ThemeTemplates::Id).await?
        }
    };
    touch_theme(db, theme_id).await?;
    template(db, id)
        .await?
        .ok_or_else(|| ThemeError::TemplateNotFound(path.to_string()))
}

async fn record_revision(
    db: &Db,
    current: &Template,
    saved_at: &str,
    saved_by: Option<i64>,
) -> Result<(), ThemeError> {
    let insert = Query::insert()
        .into_table(ThemeTemplateRevisions::Table)
        .columns([
            ThemeTemplateRevisions::TemplateId,
            ThemeTemplateRevisions::Settings,
            ThemeTemplateRevisions::Markup,
            ThemeTemplateRevisions::SavedAt,
            ThemeTemplateRevisions::SavedBy,
        ])
        .values_panic([
            current.id.into(),
            current.settings.to_string().into(),
            current.markup.clone().into(),
            saved_at.into(),
            saved_by.into(),
        ])
        .to_owned();
    insert_returning_id(db, insert, ThemeTemplateRevisions::Id).await?;
    Ok(())
}

/// Deletes a template and its revisions. Refused for a locked theme.
pub async fn delete_template(db: &Db, id: i64) -> Result<(), ThemeError> {
    let Some(current) = template(db, id).await? else {
        return Err(ThemeError::TemplateNotFound(id.to_string()));
    };
    let theme = theme_by_id(db, current.theme_id).await?;
    if theme.locked {
        return Err(ThemeError::Locked(theme.code));
    }
    let (sql, values) = build(
        db.backend,
        Query::delete()
            .from_table(ThemeTemplates::Table)
            .and_where(Expr::col(ThemeTemplates::Id).eq(id))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    touch_theme(db, theme.id).await?;
    Ok(())
}

/// A template's revisions, newest first.
pub async fn revisions(db: &Db, template_id: i64) -> Result<Vec<Revision>, ThemeError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns([
                ThemeTemplateRevisions::Id,
                ThemeTemplateRevisions::TemplateId,
                ThemeTemplateRevisions::Settings,
                ThemeTemplateRevisions::Markup,
                ThemeTemplateRevisions::SavedAt,
                ThemeTemplateRevisions::SavedBy,
            ])
            .from(ThemeTemplateRevisions::Table)
            .and_where(Expr::col(ThemeTemplateRevisions::TemplateId).eq(template_id))
            .order_by(ThemeTemplateRevisions::Id, Order::Desc)
            .to_owned(),
    );
    let rows = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?;
    rows.iter().map(revision_from_row).collect()
}

/// Puts a revision's content back as the template's current content. The
/// content it replaces becomes a revision in turn.
pub async fn restore_revision(
    db: &Db,
    revision_id: i64,
    saved_by: Option<i64>,
) -> Result<Template, ThemeError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns([
                ThemeTemplateRevisions::Id,
                ThemeTemplateRevisions::TemplateId,
                ThemeTemplateRevisions::Settings,
                ThemeTemplateRevisions::Markup,
                ThemeTemplateRevisions::SavedAt,
                ThemeTemplateRevisions::SavedBy,
            ])
            .from(ThemeTemplateRevisions::Table)
            .and_where(Expr::col(ThemeTemplateRevisions::Id).eq(revision_id))
            .to_owned(),
    );
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&db.pool)
        .await?;
    let revision = match row {
        Some(r) => revision_from_row(&r)?,
        None => return Err(ThemeError::TemplateNotFound(revision_id.to_string())),
    };
    let current = template(db, revision.template_id)
        .await?
        .ok_or_else(|| ThemeError::TemplateNotFound(revision.template_id.to_string()))?;
    save_template(
        db,
        current.theme_id,
        current.kind,
        &current.path,
        Some(revision.settings),
        &revision.markup,
        saved_by,
    )
    .await
}
