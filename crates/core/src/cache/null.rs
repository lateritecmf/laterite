//! The `null` store: keeps nothing, so every read loads afresh. Generations
//! still move and locks still hold.

use std::time::Duration;

use async_trait::async_trait;

use super::{Entry, ValueStore};
use crate::error::CoreResult;

/// A store that holds nothing.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullStore;

#[async_trait]
impl ValueStore for NullStore {
    fn name(&self) -> &'static str {
        "null"
    }
    async fn get(&self, _key: &str) -> CoreResult<Option<Entry>> {
        Ok(None)
    }
    async fn put(&self, _key: &str, _entry: Entry, _ttl: Duration) -> CoreResult<()> {
        Ok(())
    }
    async fn add(&self, _key: &str, _entry: Entry, _ttl: Duration) -> CoreResult<bool> {
        Ok(true)
    }
    async fn forget(&self, _key: &str) -> CoreResult<bool> {
        Ok(false)
    }
    async fn flush(&self) -> CoreResult<()> {
        Ok(())
    }
}
