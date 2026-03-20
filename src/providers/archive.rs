use super::BookProvider;
use crate::types::BookResult;
use anyhow::{Context, Result};
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
    client: reqwest::blocking::Client,
}

impl ArchiveProvider {
    pub fn new() -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(60))
            .user_agent(USER_AGENT)
            .build()
            .expect("failed to build HTTP client");
        Self { client }
    }
}

impl BookProvider for ArchiveProvider {
    fn name(&self) -> &str {
        "archive"
    }

    fn search(&self, query: &str) -> Result<Vec<BookResult>> {
        let encoded = urlencoding::encode(query);
        let url = format!(
            "https://archive.org/advancedsearch.php?q={}+AND+mediatype:texts&fl=identifier,title,creator,format&rows=10&output=json",
            encoded
        );

        let resp = self
            .client
            .get(&url)
            .send()
            .context("Archive.org search request failed")?;

        let body = resp
            .text()
            .context("Failed to read Archive.org search response body")?;

        let parsed: SearchResponse =
            serde_json::from_str(&body).context("Failed to parse Archive.org search JSON")?;

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

    fn download(&self, book: &BookResult, output_dir: &Path) -> Result<PathBuf> {
        let identifier = &book.download_id;

        // Fetch file listing from metadata endpoint
        let meta_url = format!("https://archive.org/metadata/{}/files", identifier);
        let resp = self
            .client
            .get(&meta_url)
            .send()
            .context("Archive.org metadata request failed")?;
        let body = resp
            .text()
            .context("Failed to read Archive.org metadata response")?;
        let meta: MetadataFilesResponse =
            serde_json::from_str(&body).context("Failed to parse Archive.org metadata JSON")?;

        let files = meta.result.unwrap_or_default();

        // Find a suitable PDF: prefer source=original
        let pdf_file = files
            .iter()
            .filter(|f| {
                f.name
                    .as_deref()
                    .map(|n| n.to_lowercase().ends_with(".pdf"))
                    .unwrap_or(false)
            })
            .filter(|f| {
                // Skip files > MAX_FILE_SIZE
                f.size
                    .as_deref()
                    .and_then(|s| s.parse::<u64>().ok())
                    .map(|sz| sz <= MAX_FILE_SIZE)
                    .unwrap_or(true)
            })
            .min_by_key(|f| {
                // Prefer original source (lower key = higher priority)
                if f.source.as_deref() == Some("original") {
                    0
                } else {
                    1
                }
            })
            .context("No downloadable PDF found for this item")?;

        let filename = pdf_file.name.as_deref().unwrap();
        let encoded_filename = urlencoding::encode(filename);
        let download_url = format!(
            "https://archive.org/download/{}/{}",
            identifier, encoded_filename
        );

        let mut resp = self
            .client
            .get(&download_url)
            .send()
            .context("Archive.org download request failed")?;

        if !resp.status().is_success() {
            anyhow::bail!(
                "Download failed with HTTP status {}",
                resp.status()
            );
        }

        let safe_title = sanitize_filename(&book.title);
        let ext = Path::new(filename)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("pdf");
        let out_path = output_dir.join(format!("{}.{}", safe_title, ext));

        let mut file =
            std::fs::File::create(&out_path).context("Failed to create output file")?;
        resp.copy_to(&mut file)
            .context("Failed to write downloaded data")?;
        file.flush()?;

        Ok(out_path)
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
