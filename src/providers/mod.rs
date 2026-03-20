pub mod libgen;
pub mod gutenberg;
pub mod archive;
pub mod annas_archive;

use anyhow::Result;
use std::path::{Path, PathBuf};
use crate::types::BookResult;
use async_trait::async_trait;

#[async_trait]
pub trait BookProvider: Send + Sync {
    fn name(&self) -> &str;
    async fn search(&self, query: &str) -> Result<Vec<BookResult>>;
    async fn download(&self, book: &BookResult, output_dir: &Path) -> Result<PathBuf>;

    /// Streaming download: sends DownloadEvent through channel
    async fn download_stream(
        &self,
        book: &BookResult,
        tx: tokio::sync::mpsc::Sender<Result<crate::types::DownloadEvent>>,
    );
}
