pub mod libgen;
pub mod gutenberg;
pub mod archive;

use anyhow::Result;
use std::path::{Path, PathBuf};
use crate::types::BookResult;

pub trait BookProvider {
    fn name(&self) -> &str;
    fn search(&self, query: &str) -> Result<Vec<BookResult>>;
    fn download(&self, book: &BookResult, output_dir: &Path) -> Result<PathBuf>;
}
