//! Browsing the library: the records in a folder, searched, filtered by kind
//! and sorted, a page at a time.

use laterite_core::strata::*;
use laterite_core::Db;
use sea_query::{Func, SelectStatement, SimpleExpr};

use crate::record::{from_row, Media, MediaRecord, RecordError, COLUMNS};
use crate::Kind;

/// How a listing is ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    #[default]
    Newest,
    Oldest,
    Name,
    Size,
}

impl Sort {
    pub fn parse(name: &str) -> Option<Sort> {
        match name {
            "newest" => Some(Sort::Newest),
            "oldest" => Some(Sort::Oldest),
            "name" => Some(Sort::Name),
            "size" => Some(Sort::Size),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Sort::Newest => "newest",
            Sort::Oldest => "oldest",
            Sort::Name => "name",
            Sort::Size => "size",
        }
    }
}

/// What a listing asks for.
#[derive(Debug, Clone, Default)]
pub struct Listing {
    /// The folder; `None` is the root. Ignored when `search` is set, which
    /// looks across the library.
    pub folder: Option<i64>,
    /// A case-insensitive match on the name, title or alt text.
    pub search: Option<String>,
    pub kind: Option<Kind>,
    pub sort: Sort,
    pub page: u64,
    pub per_page: u64,
}

/// A page of records and the whole count.
#[derive(Debug, Clone)]
pub struct Page {
    pub items: Vec<MediaRecord>,
    pub total: i64,
}

fn kind_condition(kind: Kind) -> SimpleExpr {
    let col = Expr::col(Media::ContentType);
    match kind {
        Kind::Image => col.like("image/%"),
        Kind::Video => col.like("video/%"),
        Kind::Audio => col.like("audio/%"),
        Kind::Document => Expr::col(Media::ContentType)
            .eq("application/pdf")
            .or(Expr::col(Media::ContentType).like("text/%"))
            .or(Expr::col(Media::ContentType).like("%msword%"))
            .or(Expr::col(Media::ContentType).like("%officedocument%"))
            .or(Expr::col(Media::ContentType).like("%opendocument%")),
        Kind::Other => Expr::col(Media::ContentType)
            .not_like("image/%")
            .and(Expr::col(Media::ContentType).not_like("video/%"))
            .and(Expr::col(Media::ContentType).not_like("audio/%"))
            .and(Expr::col(Media::ContentType).ne("application/pdf"))
            .and(Expr::col(Media::ContentType).not_like("text/%"))
            .and(Expr::col(Media::ContentType).not_like("%msword%"))
            .and(Expr::col(Media::ContentType).not_like("%officedocument%"))
            .and(Expr::col(Media::ContentType).not_like("%opendocument%")),
    }
}

fn apply(listing: &Listing, query: &mut SelectStatement) {
    query.and_where(Expr::col(Media::InLibrary).eq(1));
    match &listing.search {
        Some(term) if !term.trim().is_empty() => {
            let pattern = format!("%{}%", term.trim().to_lowercase());
            query.and_where(
                Expr::expr(Func::lower(Expr::col(Media::OriginalName)))
                    .like(pattern.clone())
                    .or(Expr::expr(Func::lower(Expr::col(Media::Title))).like(pattern.clone()))
                    .or(Expr::expr(Func::lower(Expr::col(Media::Alt))).like(pattern)),
            );
        }
        _ => match listing.folder {
            Some(id) => {
                query.and_where(Expr::col(Media::FolderId).eq(id));
            }
            None => {
                query.and_where(Expr::col(Media::FolderId).is_null());
            }
        },
    }
    if let Some(kind) = listing.kind {
        query.and_where(kind_condition(kind));
    }
}

/// One page of the library.
pub async fn list(db: &Db, listing: &Listing) -> Result<Page, RecordError> {
    let per_page = listing.per_page.clamp(1, 500);
    let page = listing.page.max(1);
    let (sql, values) = {
        let mut query = Query::select()
            .expr(Expr::col(Media::Id).count())
            .from(Media::Table)
            .to_owned();
        apply(listing, &mut query);
        build(db.backend, query)
    };
    let total: i64 = bind_values_as(sqlx::query_as::<_, (i64,)>(&sql), values)
        .fetch_one(&db.pool)
        .await?
        .0;
    let (sql, values) = {
        let mut query = Query::select()
            .columns(COLUMNS)
            .from(Media::Table)
            .to_owned();
        apply(listing, &mut query);
        match listing.sort {
            Sort::Newest => query.order_by(Media::Id, Order::Desc),
            Sort::Oldest => query.order_by(Media::Id, Order::Asc),
            Sort::Name => query
                .order_by(Media::OriginalName, Order::Asc)
                .order_by(Media::Id, Order::Asc),
            Sort::Size => query
                .order_by(Media::Size, Order::Desc)
                .order_by(Media::Id, Order::Asc),
        };
        query.limit(per_page).offset((page - 1) * per_page);
        build(db.backend, query)
    };
    let rows = bind_values(sqlx::query(&sql), values)
        .fetch_all(&db.pool)
        .await?;
    let items = rows.iter().map(from_row).collect::<Result<_, _>>()?;
    Ok(Page { items, total })
}
