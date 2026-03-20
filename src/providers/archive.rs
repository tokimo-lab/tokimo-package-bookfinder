use super::BookProvider;
use crate::types::BookResult;
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 \
    (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
const MAX_FILE_SIZE: u64 = 100 * 1024 * 1024; // 100 MB

// --- Search response structs ---

#[derive(Debug, Deserialize)]
struct SearchResponse {
    response: SearchInner,
}

#[derive(Debug, Deserialize)]
struct SearchInner {
    docs: Vec<SearchDoc>,
}

#[derive(Debug, Deserialize)]
struct SearchDoc {
    identifier: String,
    title: Option<String>,
    creator: Option<CreatorField>,
    format: Option<FormatField>,
}

/// `creator` can be a single string or an array of strings.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum CreatorField {
    Single(String),
    Multiple(Vec<String>),
}

/// `format` can be a single string or an array of strings.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum FormatField {
    Single(String),
    Multiple(Vec<String>),
}

// --- Metadata / files response structs ---

#[derive(Debug, Deserialize)]
struct MetadataFilesResponse {
    result: Option<Vec<FileEntry>>,
}

#[derive(Debug, Deserialize)]
struct FileEntry {
    name: Option<String>,
    source: Option<String>,
    size: Option<String>,
}

pub struct ArchiveProvider {
    client: reqwest::Client,
}

impl ArchiveProvider {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .user_agent(USER_AGENT)
            .build()
            .expect("failed to build HTTP client");
        Self { client }
    }
}

pub fn parse_archive_json(json: &str) -> Result<Vec<BookResult>> {
    let parsed: SearchResponse =
        serde_json::from_str(json).context("Failed to parse Archive.org search JSON")?;

    let results = parsed
        .response
        .docs
        .into_iter()
        .map(|doc| {
            let title = doc.title.unwrap_or_else(|| doc.identifier.clone());
            let author = match doc.creator {
                Some(CreatorField::Single(s)) => s,
                Some(CreatorField::Multiple(v)) => v.join(", "),
                None => "Unknown".to_string(),
            };
            let format = detect_format(&doc.format);
            BookResult {
                id: doc.identifier.clone(),
                title,
                author,
                format,
                size: "unknown".to_string(),
                provider: "archive".to_string(),
                download_id: doc.identifier,
            }
        })
        .collect();

    Ok(results)
}

/// Resolve the download file name and URL from archive.org metadata.
async fn resolve_archive_download(
    client: &reqwest::Client,
    book: &BookResult,
) -> Result<(String, String)> {
    let identifier = &book.download_id;

    let meta_url = format!("https://archive.org/metadata/{}/files", identifier);
    let resp = client.get(&meta_url).send().await
        .context("Archive.org metadata request failed")?;
    let body = resp.text().await
        .context("Failed to read Archive.org metadata response")?;
    let meta: MetadataFilesResponse =
        serde_json::from_str(&body).context("Failed to parse Archive.org metadata JSON")?;

    let files = meta.result.unwrap_or_default();

    let downloadable_extensions = ["pdf", "epub"];
    let pdf_file = downloadable_extensions
        .iter()
        .find_map(|target_ext| {
            files
                .iter()
                .filter(|f| {
                    f.name
                        .as_deref()
                        .map(|n| n.to_lowercase().ends_with(&format!(".{}", target_ext)))
                        .unwrap_or(false)
                })
                .filter(|f| {
                    f.size
                        .as_deref()
                        .and_then(|s| s.parse::<u64>().ok())
                        .map(|sz| sz <= MAX_FILE_SIZE)
                        .unwrap_or(true)
                })
                .min_by_key(|f| {
                    if f.source.as_deref() == Some("original") { 0 } else { 1 }
                })
        })
        .context("No downloadable PDF or EPUB found for this item")?;

    let filename = pdf_file.name.as_deref().unwrap().to_string();
    let encoded_filename = urlencoding::encode(&filename);
    let download_url = format!(
        "https://archive.org/download/{}/{}",
        identifier, encoded_filename
    );

    Ok((filename, download_url))
}

#[async_trait]
impl BookProvider for ArchiveProvider {
    fn name(&self) -> &str {
        "archive"
    }

    async fn search(&self, query: &str) -> Result<Vec<BookResult>> {
        let encoded = urlencoding::encode(query);
        let url = format!(
            "https://archive.org/advancedsearch.php?q={}+AND+mediatype:texts+AND+collection:opensource&fl=identifier,title,creator,format&rows=15&output=json",
            encoded
        );

        let resp = self.client.get(&url).send().await
            .context("Archive.org search request failed")?;
        let body = resp.text().await
            .context("Failed to read Archive.org search response body")?;

        parse_archive_json(&body)
    }

    async fn download(&self, book: &BookResult, output_dir: &Path) -> Result<PathBuf> {
        let identifier = &book.download_id;
        let (filename, download_url) = resolve_archive_download(&self.client, book).await?;

        let resp = self.client.get(&download_url).send().await
            .context("Archive.org download request failed")?;

        if !resp.status().is_success() {
            // Try alternate: some items have _text.pdf derivative
            let alt_url = format!(
                "https://archive.org/download/{}/{}_text.pdf",
                identifier, identifier
            );
            let alt_resp = self.client.get(&alt_url).send().await;
            if let Ok(r) = alt_resp {
                if r.status().is_success() {
                    let safe_title = sanitize_filename(&book.title);
                    let out_path = output_dir.join(format!("{}.pdf", safe_title));
                    let mut file = std::fs::File::create(&out_path).context("Failed to create output file")?;
                    let bytes = r.bytes().await.context("Failed to read download bytes")?;
                    file.write_all(&bytes)?;
                    return Ok(out_path);
                }
            }
            anyhow::bail!(
                "Download failed with HTTP status {} (this item may require borrowing)",
                resp.status()
            );
        }

        let safe_title = sanitize_filename(&book.title);
        let ext = Path::new(&filename)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("pdf");
        let out_path = output_dir.join(format!("{}.{}", safe_title, ext));

        let bytes = resp.bytes().await.context("Failed to read downloaded data")?;
        let mut file = std::fs::File::create(&out_path).context("Failed to create output file")?;
        file.write_all(&bytes)?;
        file.flush()?;

        Ok(out_path)
    }

    async fn download_stream(
        &self,
        book: &BookResult,
        tx: tokio::sync::mpsc::Sender<Result<crate::types::DownloadEvent>>,
    ) {
        use crate::types::DownloadEvent;

        macro_rules! send {
            ($val:expr) => { if tx.send($val).await.is_err() { return; } };
        }

        let identifier = &book.download_id;

        let (resolved_filename, download_url) = match resolve_archive_download(&self.client, book).await {
            Ok(v) => v,
            Err(e) => { send!(Err(e)); return; }
        };

        let resp_result = self.client.get(&download_url).send().await;
        let mut resp = match resp_result {
            Ok(r) if r.status().is_success() => r,
            Ok(r) => {
                // Try alternate URL
                let alt_url = format!(
                    "https://archive.org/download/{}/{}_text.pdf",
                    identifier, identifier
                );
                match self.client.get(&alt_url).send().await {
                    Ok(alt_r) if alt_r.status().is_success() => alt_r,
                    _ => {
                        send!(Err(anyhow::anyhow!(
                            "Download failed with HTTP status {} (this item may require borrowing)",
                            r.status()
                        )));
                        return;
                    }
                }
            }
            Err(e) => { send!(Err(e.into())); return; }
        };

        let total_bytes = resp.content_length();
        let ext = Path::new(&resolved_filename)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("pdf");
        let safe_title = sanitize_filename(&book.title);
        let filename = format!("{}.{}", safe_title, ext);

        send!(Ok(DownloadEvent::FileInfo {
            title: book.title.clone(),
            author: book.author.clone(),
            format: book.format.clone(),
            filename: filename.clone(),
            total_bytes,
        }));

        let mut downloaded = 0u64;
        loop {
            match resp.chunk().await {
                Ok(Some(chunk)) => {
                    downloaded += chunk.len() as u64;
                    send!(Ok(DownloadEvent::Data { bytes: chunk.to_vec(), downloaded }));
                }
                Ok(None) => break,
                Err(e) => { send!(Err(e.into())); return; }
            }
        }

        send!(Ok(DownloadEvent::Done { filename, total_bytes: downloaded }));
    }
}

fn detect_format(field: &Option<FormatField>) -> String {
    let formats: Vec<String> = match field {
        Some(FormatField::Single(s)) => vec![s.to_lowercase()],
        Some(FormatField::Multiple(v)) => v.iter().map(|s| s.to_lowercase()).collect(),
        None => return "pdf".to_string(),
    };
    for f in &formats {
        if f.contains("pdf") {
            return "pdf".to_string();
        }
    }
    for f in &formats {
        if f.contains("epub") {
            return "epub".to_string();
        }
    }
    "pdf".to_string()
}

fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' { c } else { '_' })
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_archive_json_single_result() {
        let json = r#"{"response": {"docs": [{"identifier": "test-book-123", "title": "Test Book", "creator": "Test Author", "format": ["PDF"]}]}}"#;
        let results = parse_archive_json(json).unwrap();
        assert_eq!(results.len(), 1);
        let book = &results[0];
        assert_eq!(book.id, "test-book-123");
        assert_eq!(book.title, "Test Book");
        assert_eq!(book.author, "Test Author");
        assert_eq!(book.format, "pdf");
        assert_eq!(book.provider, "archive");
    }

    #[test]
    fn test_parse_archive_json_multiple_creators() {
        let json = r#"{"response": {"docs": [{"identifier": "multi-author", "title": "Multi Author Book", "creator": ["Alice", "Bob"], "format": ["EPUB"]}]}}"#;
        let results = parse_archive_json(json).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].author, "Alice, Bob");
        assert_eq!(results[0].format, "epub");
    }

    #[test]
    fn test_parse_archive_json_missing_optional_fields() {
        let json = r#"{"response": {"docs": [{"identifier": "no-title-no-author"}]}}"#;
        let results = parse_archive_json(json).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "no-title-no-author", "identifier used as title fallback");
        assert_eq!(results[0].author, "Unknown");
    }

    #[test]
    fn test_parse_archive_json_empty_docs() {
        let json = r#"{"response": {"docs": []}}"#;
        let results = parse_archive_json(json).unwrap();
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_parse_archive_json_invalid_returns_err() {
        let result = parse_archive_json("not valid json");
        assert!(result.is_err());
    }
}
