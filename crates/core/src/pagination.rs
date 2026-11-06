//! Pagination primitives shared by admin lists and API endpoints.

use serde::{Deserialize, Serialize};

/// Largest page size a request may ask for; larger requests are clamped.
pub const MAX_PER_PAGE: u32 = 200;
/// Page size a request gets when it names none.
pub const DEFAULT_PER_PAGE: u32 = 25;

/// An incoming page request (1-based).
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct PageRequest {
    /// Page number, from 1. Default `1`.
    #[serde(default = "default_page")]
    pub page: u32,
    /// Items per page. Default [`DEFAULT_PER_PAGE`].
    #[serde(default = "default_per_page")]
    pub per_page: u32,
}

fn default_page() -> u32 {
    1
}

fn default_per_page() -> u32 {
    DEFAULT_PER_PAGE
}

impl Default for PageRequest {
    fn default() -> Self {
        Self {
            page: 1,
            per_page: DEFAULT_PER_PAGE,
        }
    }
}

impl PageRequest {
    /// Clamps page and page size into sane bounds.
    pub fn clamped(self) -> Self {
        Self {
            page: self.page.max(1),
            per_page: self.per_page.clamp(1, MAX_PER_PAGE),
        }
    }

    /// Returns the clamped page size as a SQL `LIMIT`.
    pub fn limit(&self) -> i64 {
        i64::from(self.clamped().per_page)
    }

    /// Returns the number of rows before the clamped page, as a SQL `OFFSET`.
    pub fn offset(&self) -> i64 {
        let clamped = self.clamped();
        i64::from(clamped.page - 1) * i64::from(clamped.per_page)
    }
}

/// One page of results with totals for rendering pagers.
#[derive(Debug, Clone, Serialize)]
pub struct Page<T> {
    /// The items on this page.
    pub items: Vec<T>,
    /// Page number, from 1.
    pub page: u32,
    /// Items per page.
    pub per_page: u32,
    /// Total number of items across every page.
    pub total: u64,
}

impl<T> Page<T> {
    /// Returns how many pages the total fills; `0` when `per_page` is `0`.
    pub fn total_pages(&self) -> u32 {
        if self.per_page == 0 {
            return 0;
        }
        let pages = self.total.div_ceil(u64::from(self.per_page));
        u32::try_from(pages).unwrap_or(u32::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_out_of_range_requests() {
        let request = PageRequest {
            page: 0,
            per_page: 10_000,
        };
        let clamped = request.clamped();
        assert_eq!(clamped.page, 1);
        assert_eq!(clamped.per_page, MAX_PER_PAGE);
    }

    #[test]
    fn computes_limit_and_offset() {
        let request = PageRequest {
            page: 3,
            per_page: 25,
        };
        assert_eq!(request.limit(), 25);
        assert_eq!(request.offset(), 50);
    }

    #[test]
    fn computes_total_pages() {
        let page = Page::<u8> {
            items: vec![],
            page: 1,
            per_page: 25,
            total: 51,
        };
        assert_eq!(page.total_pages(), 3);
    }
}
