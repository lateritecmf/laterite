//! The theme tables' identifiers.

use laterite_core::strata::Iden;

/// A theme: a code, its parent, whether it is locked, and its YAML config.
#[derive(Iden)]
pub(crate) enum Themes {
    Table,
    Id,
    Code,
    Parent,
    Locked,
    Config,
    CreatedAt,
    UpdatedAt,
}

/// A theme's templates: one row per kind and path.
#[derive(Iden)]
pub(crate) enum ThemeTemplates {
    Table,
    Id,
    ThemeId,
    Kind,
    Path,
    Settings,
    Markup,
    CreatedAt,
    UpdatedAt,
}

/// What a template held before each save, so an edit can be undone.
#[derive(Iden)]
pub(crate) enum ThemeTemplateRevisions {
    Table,
    Id,
    TemplateId,
    Settings,
    Markup,
    SavedAt,
    SavedBy,
}
