pub mod providers;
pub mod types;

pub use providers::BookProvider;
pub use types::{BookResult, DownloadEvent};

use futures::Stream;
use tokio_stream::wrappers::ReceiverStream;

/// Returns all available providers.
pub fn get_providers() -> Vec<Box<dyn BookProvider>> {
    vec![
        Box::new(providers::libgen::LibgenProvider::new()),
        Box::new(providers::gutenberg::GutenbergProvider::new()),
        Box::new(providers::archive::ArchiveProvider::new()),
        Box::new(providers::annas_archive::AnnasArchiveProvider::new("")),
    ]
}

/// List provider names.
pub fn list_provider_names() -> Vec<&'static str> {
    vec!["libgen", "gutenberg", "archive", "annas-archive"]
}

/// Stream search results from all (or specified) providers.
/// Each provider runs as a spawned task; results arrive as each provider responds.
pub fn search_stream(query: impl Into<String>) -> impl Stream<Item = BookResult> + Send + 'static {
    let query = query.into();
    let (tx, rx) = tokio::sync::mpsc::channel::<BookResult>(256);

    let providers = get_providers();
    for provider in providers {
        let tx = tx.clone();
        let q = query.clone();
        tokio::spawn(async move {
            if let Ok(Ok(results)) =
                tokio::time::timeout(std::time::Duration::from_secs(30), provider.search(&q)).await
            {
                for r in results {
                    if tx.send(r).await.is_err() {
                        break;
                    }
                }
            }
        });
    }
    drop(tx);
    ReceiverStream::new(rx)
}

/// Stream download events for a specific book.
pub fn download_stream(
    provider_name: impl Into<String>,
    book: BookResult,
) -> impl Stream<Item = anyhow::Result<DownloadEvent>> + Send + 'static {
    let provider_name = provider_name.into();
    let (tx, rx) = tokio::sync::mpsc::channel::<anyhow::Result<DownloadEvent>>(64);

    tokio::spawn(async move {
        let providers = get_providers();
        let provider = match providers.into_iter().find(|p| p.name() == provider_name) {
            Some(p) => p,
            None => {
                let _ = tx
                    .send(Err(anyhow::anyhow!(
                        "Provider '{}' not found",
                        provider_name
                    )))
                    .await;
                return;
            }
        };
        provider.download_stream(&book, tx).await;
    });

    ReceiverStream::new(rx)
}
