use std::sync::Arc;

use async_trait::async_trait;
use axiomcli::{
    Result,
    tools::{ToolRegistry, registry_with_search},
    web::{SearchProvider, SearchReport},
};
use tokio_util::sync::CancellationToken;

struct UnexpectedSearch;

#[async_trait]
impl SearchProvider for UnexpectedSearch {
    fn provenance(&self) -> &'static str {
        "https://search.invalid/"
    }

    async fn search(&self, _query: &str, _cancellation: CancellationToken) -> Result<SearchReport> {
        panic!("this fixture must not issue web searches")
    }
}

pub fn test_registry(max_output_bytes: usize) -> Result<ToolRegistry> {
    registry_with_search(max_output_bytes, Arc::new(UnexpectedSearch))
}
