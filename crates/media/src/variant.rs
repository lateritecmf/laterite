//! Variants: named, cached derivatives of a media record (a `thumb`, a
//! `card`), each a row pointing at its own blob path.

use laterite_core::strata::*;

#[derive(Iden)]
pub enum MediaVariants {
    Table,
    Id,
    MediaId,
    Preset,
    Path,
    Width,
    Height,
    Size,
    ContentType,
    CreatedAt,
}
