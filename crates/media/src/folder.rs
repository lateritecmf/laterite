//! Library folders: rows, never disk directories. A folder has a parent and a
//! materialized path (`/photos/2026`), so a subtree is one prefix query and
//! browsing never asks the storage driver to list anything.

use chrono::{SecondsFormat, Utc};
use laterite_core::strata::*;
use laterite_core::Db;

use crate::record::{Media, RecordError};

#[derive(Iden)]
pub enum MediaFolders {
    Table,
    Id,
    ParentId,
    Name,
    Path,
    CreatedAt,
    UpdatedAt,
}

/// A folder row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
    /// `/a/b` for the folder `b` under `a`; the root is `/`.
    pub path: String,
}

/// Why a folder operation was refused.
#[derive(Debug, thiserror::Error)]
pub enum FolderError {
    #[error("database error")]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Record(#[from] RecordError),
    #[error("{0:?} is not a folder name")]
    Name(String),
    #[error("a folder named {0:?} is already there")]
    Taken(String),
    #[error("no folder {0}")]
    NotFound(i64),
    #[error("a folder cannot move into itself or below")]
    Cycle,
    #[error("the folder still holds {files} file(s) and {folders} folder(s)")]
    NotEmpty { files: i64, folders: i64 },
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)
}

/// A folder name: no slash, no control character, trimmed, at most 120 characters.
pub fn valid_name(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty()
        && name.len() <= 120
        && name != "."
        && name != ".."
        && !name
            .chars()
            .any(|c| c == '/' || c == '\\' || c.is_control())
}

const COLUMNS: [MediaFolders; 4] = [
    MediaFolders::Id,
    MediaFolders::ParentId,
    MediaFolders::Name,
    MediaFolders::Path,
];

fn from_row(r: &sqlx::any::AnyRow) -> Result<Folder, sqlx::Error> {
    Ok(Folder {
        id: r.get_int("id")?,
        parent_id: r.get_int_opt("parent_id")?,
        name: r.get_text("name")?,
        path: r.get_text("path")?,
    })
}

/// A folder by id.
pub async fn find(db: &Db, id: i64) -> Result<Option<Folder>, FolderError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns(COLUMNS)
            .from(MediaFolders::Table)
            .and_where(Expr::col(MediaFolders::Id).eq(id))
            .to_owned(),
    );
    let row = bind_values(sqlx::query(&sql), values)
        .fetch_optional(&db.pool)
        .await?;
    Ok(row.as_ref().map(from_row).transpose()?)
}

async fn require(db: &Db, id: i64) -> Result<Folder, FolderError> {
    find(db, id).await?.ok_or(FolderError::NotFound(id))
}

/// The folders directly under `parent` (`None` for the root), by name.
pub async fn children(db: &Db, parent: Option<i64>) -> Result<Vec<Folder>, FolderError> {
    // Built and rendered in one block, so the builder never crosses an await.
    let (sql, values) = {
        let mut query = Query::select()
            .columns(COLUMNS)
            .from(MediaFolders::Table)
            .order_by(MediaFolders::Name, Order::Asc)
            .to_owned();
        match parent {
            Some(id) => {
                query.and_where(Expr::col(MediaFolders::ParentId).eq(id));
            }
            None => {
                query.and_where(Expr::col(MediaFolders::ParentId).is_null());
            }
        }
        build(db.backend, query)
    };
    let rows = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?;
    Ok(rows.iter().map(from_row).collect::<Result<_, _>>()?)
}

/// Every folder, by path, for a tree or a picker.
pub async fn all(db: &Db) -> Result<Vec<Folder>, FolderError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .columns(COLUMNS)
            .from(MediaFolders::Table)
            .order_by(MediaFolders::Path, Order::Asc)
            .to_owned(),
    );
    let rows = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?;
    Ok(rows.iter().map(from_row).collect::<Result<_, _>>()?)
}

/// The way down to a folder: the root's children first, the folder last.
pub async fn ancestors(db: &Db, id: i64) -> Result<Vec<Folder>, FolderError> {
    let mut chain = Vec::new();
    let mut next = Some(id);
    while let Some(current) = next {
        let folder = require(db, current).await?;
        next = folder.parent_id;
        chain.push(folder);
        if chain.len() > 64 {
            break;
        }
    }
    chain.reverse();
    Ok(chain)
}

async fn sibling_taken(
    db: &Db,
    parent: Option<i64>,
    name: &str,
    except: Option<i64>,
) -> Result<bool, FolderError> {
    Ok(children(db, parent)
        .await?
        .iter()
        .any(|f| f.name.eq_ignore_ascii_case(name) && Some(f.id) != except))
}

/// Creates a folder under `parent` (`None` for the root).
pub async fn create(db: &Db, parent: Option<i64>, name: &str) -> Result<Folder, FolderError> {
    let name = name.trim();
    if !valid_name(name) {
        return Err(FolderError::Name(name.to_string()));
    }
    let parent_path = match parent {
        Some(id) => require(db, id).await?.path,
        None => String::new(),
    };
    if sibling_taken(db, parent, name, None).await? {
        return Err(FolderError::Taken(name.to_string()));
    }
    let path = format!("{}/{name}", parent_path.trim_end_matches('/'));
    let now = now();
    let stmt = Query::insert()
        .into_table(MediaFolders::Table)
        .columns([
            MediaFolders::ParentId,
            MediaFolders::Name,
            MediaFolders::Path,
            MediaFolders::CreatedAt,
            MediaFolders::UpdatedAt,
        ])
        .values_panic([
            parent.into(),
            name.into(),
            path.into(),
            now.clone().into(),
            now.into(),
        ])
        .to_owned();
    let id = insert_returning_id(db, stmt, MediaFolders::Id).await?;
    require(db, id).await
}

/// Rewrites a folder's path and every descendant's after a rename or a move.
async fn repath(db: &Db, folder: &Folder, new_path: &str) -> Result<(), FolderError> {
    let old_prefix = format!("{}/", folder.path);
    let new_prefix = format!("{}/", new_path);
    // The folder itself.
    let (sql, values) = build(
        db.backend,
        Query::update()
            .table(MediaFolders::Table)
            .value(MediaFolders::Path, new_path)
            .value(MediaFolders::UpdatedAt, now())
            .and_where(Expr::col(MediaFolders::Id).eq(folder.id))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    // Its descendants, by prefix.
    for f in all(db).await? {
        if let Some(rest) = f.path.strip_prefix(&old_prefix) {
            let path = format!("{new_prefix}{rest}");
            let (sql, values) = build(
                db.backend,
                Query::update()
                    .table(MediaFolders::Table)
                    .value(MediaFolders::Path, path)
                    .value(MediaFolders::UpdatedAt, now())
                    .and_where(Expr::col(MediaFolders::Id).eq(f.id))
                    .to_owned(),
            );
            bind_values(sqlx::query(&sql), values)
                .execute(&db.pool)
                .await?;
        }
    }
    Ok(())
}

/// Renames a folder.
pub async fn rename(db: &Db, id: i64, name: &str) -> Result<Folder, FolderError> {
    let name = name.trim();
    if !valid_name(name) {
        return Err(FolderError::Name(name.to_string()));
    }
    let folder = require(db, id).await?;
    if sibling_taken(db, folder.parent_id, name, Some(id)).await? {
        return Err(FolderError::Taken(name.to_string()));
    }
    let parent_path = folder.path.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
    let new_path = format!("{parent_path}/{name}");
    let (sql, values) = build(
        db.backend,
        Query::update()
            .table(MediaFolders::Table)
            .value(MediaFolders::Name, name)
            .and_where(Expr::col(MediaFolders::Id).eq(id))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    repath(db, &folder, &new_path).await?;
    require(db, id).await
}

/// Moves a folder under `parent` (`None` for the root). Refused into itself
/// or a descendant.
pub async fn move_to(db: &Db, id: i64, parent: Option<i64>) -> Result<Folder, FolderError> {
    let folder = require(db, id).await?;
    let parent_path = match parent {
        Some(pid) => {
            let target = require(db, pid).await?;
            if target.id == folder.id || target.path.starts_with(&format!("{}/", folder.path)) {
                return Err(FolderError::Cycle);
            }
            target.path
        }
        None => String::new(),
    };
    if sibling_taken(db, parent, &folder.name, Some(id)).await? {
        return Err(FolderError::Taken(folder.name.clone()));
    }
    let (sql, values) = build(
        db.backend,
        Query::update()
            .table(MediaFolders::Table)
            .value(MediaFolders::ParentId, parent)
            .and_where(Expr::col(MediaFolders::Id).eq(id))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    let new_path = format!("{}/{}", parent_path.trim_end_matches('/'), folder.name);
    repath(db, &folder, &new_path).await?;
    require(db, id).await
}

/// How many files and folders a folder holds directly.
pub async fn contents(db: &Db, id: i64) -> Result<(i64, i64), FolderError> {
    let (sql, values) = build(
        db.backend,
        Query::select()
            .expr(Expr::col(Media::Id).count())
            .from(Media::Table)
            .and_where(Expr::col(Media::FolderId).eq(id))
            .to_owned(),
    );
    let files: i64 = bind_values_as(sqlx::query_as::<_, (i64,)>(&sql), values)
        .fetch_one(&db.pool)
        .await?
        .0;
    let (sql, values) = build(
        db.backend,
        Query::select()
            .expr(Expr::col(MediaFolders::Id).count())
            .from(MediaFolders::Table)
            .and_where(Expr::col(MediaFolders::ParentId).eq(id))
            .to_owned(),
    );
    let folders: i64 = bind_values_as(sqlx::query_as::<_, (i64,)>(&sql), values)
        .fetch_one(&db.pool)
        .await?
        .0;
    Ok((files, folders))
}

/// Deletes an empty folder.
pub async fn delete(db: &Db, id: i64) -> Result<(), FolderError> {
    require(db, id).await?;
    let (files, folders) = contents(db, id).await?;
    if files > 0 || folders > 0 {
        return Err(FolderError::NotEmpty { files, folders });
    }
    let (sql, values) = build(
        db.backend,
        Query::delete()
            .from_table(MediaFolders::Table)
            .and_where(Expr::col(MediaFolders::Id).eq(id))
            .to_owned(),
    );
    bind_values(sqlx::query(&sql), values)
        .execute(&db.pool)
        .await?;
    Ok(())
}
