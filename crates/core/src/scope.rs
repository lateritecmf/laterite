//! Query hooks: named conditions a module adds to every read of an entity.
//!
//! A hook registered for an entity shapes each admin read of it (list rows and
//! count, the edit form's load, export, relation sections, reordering) through
//! one function, [`apply`]. A read lifts a hook by its name, the way the trash
//! view lifts [`SoftDelete`].

use std::sync::Arc;

use sea_query::{Alias, Expr, SelectStatement};

use crate::migration::{key_col, DbBackend};
use crate::record::ListenerTarget;

/// The column a trashed row carries its trash time in; null while it is live.
pub const DELETED_AT: &str = "deleted_at";

/// What a hook knows about the read it shapes.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct ScopeCx<'a> {
    /// The table being read, as the select names it.
    pub entity: &'a str,
    /// The database the select runs against.
    pub backend: DbBackend,
}

impl<'a> ScopeCx<'a> {
    /// The context for a read of `entity` on `backend`.
    pub fn new(entity: &'a str, backend: DbBackend) -> Self {
        Self { entity, backend }
    }
}

/// A named condition added to every read of the entities it targets.
///
/// `apply` only adds to the select (conditions, joins); it never replaces what
/// the read already chose, and it runs once per read.
pub trait QueryHook: Send + Sync + 'static {
    /// The name a read lifts the hook by, a dotted `vendor.name` for a module's
    /// own hook.
    fn name(&self) -> &str;
    /// Adds the hook's conditions to `select`.
    fn apply(&self, cx: &ScopeCx<'_>, select: &mut SelectStatement);
}

/// A registered query hook and the entities it shapes.
#[derive(Clone)]
pub struct QueryHookReg {
    /// Which entities the hook applies to.
    pub target: ListenerTarget,
    /// The hook itself.
    pub hook: Arc<dyn QueryHook>,
}

impl QueryHookReg {
    /// A hook for one entity.
    pub fn for_entity(entity: impl Into<String>, hook: Arc<dyn QueryHook>) -> Self {
        Self {
            target: ListenerTarget::Entity(entity.into()),
            hook,
        }
    }

    /// A hook for every entity.
    pub fn all(hook: Arc<dyn QueryHook>) -> Self {
        Self {
            target: ListenerTarget::All,
            hook,
        }
    }

    /// Whether the hook applies to `entity`.
    pub fn matches(&self, entity: &str) -> bool {
        self.target.matches(entity)
    }
}

/// Adds every hook in `hooks` to `select`, except those named in `lifted`.
pub fn apply(
    hooks: &[Arc<dyn QueryHook>],
    cx: &ScopeCx<'_>,
    select: &mut SelectStatement,
    lifted: &[&str],
) {
    for hook in hooks {
        if !lifted.contains(&hook.name()) {
            hook.apply(cx, select);
        }
    }
}

/// Hides trashed rows: those whose `deleted_at` is set. Named `soft_delete`.
pub struct SoftDelete;

impl SoftDelete {
    /// The name the trash view lifts this hook by.
    pub const NAME: &'static str = "soft_delete";
}

impl QueryHook for SoftDelete {
    fn name(&self) -> &str {
        Self::NAME
    }

    fn apply(&self, cx: &ScopeCx<'_>, select: &mut SelectStatement) {
        select.and_where(Expr::col((Alias::new(cx.entity), Alias::new(DELETED_AT))).is_null());
    }
}

/// The nullable `deleted_at` column a soft-deleting table adds; index it with
/// the table's other filters.
pub fn soft_delete_col() -> sea_query::ColumnDef {
    key_col(Alias::new(DELETED_AT)).null().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_query::{Query, SqliteQueryBuilder};

    struct Published;

    impl QueryHook for Published {
        fn name(&self) -> &str {
            "acme.published"
        }
        fn apply(&self, cx: &ScopeCx<'_>, select: &mut SelectStatement) {
            select.and_where(Expr::col((Alias::new(cx.entity), Alias::new("published"))).eq(1));
        }
    }

    fn sql(lifted: &[&str]) -> String {
        let hooks: Vec<Arc<dyn QueryHook>> = vec![Arc::new(SoftDelete), Arc::new(Published)];
        let mut select = Query::select();
        select.column(Alias::new("id")).from(Alias::new("posts"));
        apply(
            &hooks,
            &ScopeCx::new("posts", DbBackend::Sqlite),
            &mut select,
            lifted,
        );
        select.to_string(SqliteQueryBuilder)
    }

    #[test]
    fn every_hook_adds_its_condition() {
        let sql = sql(&[]);
        assert!(sql.contains(r#""posts"."deleted_at" IS NULL"#), "{sql}");
        assert!(sql.contains(r#""posts"."published" = 1"#), "{sql}");
    }

    #[test]
    fn a_lifted_hook_is_left_out() {
        let sql = sql(&[SoftDelete::NAME]);
        assert!(!sql.contains("deleted_at"), "{sql}");
        assert!(sql.contains("published"), "{sql}");
    }
}
