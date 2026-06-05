use super::BookProvider;
use crate::types::BookResult;
use anyhow::{Context, Result};
use async_trait::async_trait;
use regex::Regex;
use reqwest::Client;
use scraper::{Html, Selector};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct LibgenProvider {
    client: Client,
}

impl Default for LibgenProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl LibgenProvider {
    pub fn new() -> Self {
        let client = Client::builder()
            .danger_accept_invalid_certs(true)
            .timeout(Duration::from_secs(60))
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .build()
            .expect("Failed to build HTTP client");
        Self { client }
    }
}

const KNOWN_EXTENSIONS: &[&str] = &[
    "pdf", "epub", "djvu", "mobi", "chm", "azw3", "fb2", "doc", "docx", "rtf", "txt", "lit", "azw",
];

pub fn parse_libgen_html(html: &str) -> Vec<BookResult> {
    let document = Html::parse_document(html);

    let tr_sel = Selector::parse("table tr").unwrap();
    let td_sel = Selector::parse("td").unwrap();
    let a_sel = Selector::parse("a").unwrap();
    let size_re = Regex::new(r"(\d+[\.,]?\d*)\s*(kB|KB|Kb|MB|Mb|mB|GB|Gb|gB|bytes?)").unwrap();
    let isbn_re = Regex::new(r"\s+\d{10,13}[;\s].*$").unwrap();

    let mut results = Vec::new();

    for row in document.select(&tr_sel) {
        // Look for an <a> tag whose href contains "/ads.php?md5="
        let mut md5 = None;
        for a in row.select(&a_sel) {
            if let Some(href) = a.value().attr("href") {
                if href.contains("/ads.php?md5=") || href.contains("ads.php?md5=") {
                    if let Some(pos) = href.find("md5=") {
                        md5 = Some(href[pos + 4..].to_string());
                    }
                }
            }
        }
        let md5 = match md5 {
            Some(m) if !m.is_empty() => m,
            _ => continue,
        };

        let cells: Vec<String> = row
            .select(&td_sel)
            .map(|td| {
                td.text()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect();

        if cells.is_empty() {
            continue;
        }

        // Title: first cell text longer than 20 chars, truncated to reasonable length
        let title = cells
            .iter()
            .find(|c| c.len() > 20)
            .map(|t| {
                // Strip trailing ISBN-like numbers and noise
                let cleaned = isbn_re.replace(t, "").to_string();
                cleaned.chars().take(120).collect::<String>()
            })
            .unwrap_or_else(|| cells.first().cloned().unwrap_or_default());

        // Author: typically second cell, but use first non-title cell that isn't too short
        let author = cells
            .iter()
            .skip(1)
            .find(|c| !c.is_empty() && c.len() > 1 && *c != &title)
            .cloned()
            .unwrap_or_default();

        // Extension: search cells in reverse for a known extension
        let mut extension = String::new();
        'ext: for cell in cells.iter().rev() {
            let lower = cell.to_lowercase();
            for ext in KNOWN_EXTENSIONS {
                if lower == *ext || lower.starts_with(ext) {
                    extension = ext.to_string();
                    break 'ext;
                }
            }
        }

        // Size: match pattern like "3 MB" or "450 kB"
        let mut size = String::new();
        for cell in &cells {
            if let Some(cap) = size_re.captures(cell) {
                size = cap[0].to_string();
                break;
            }
        }

        results.push(BookResult {
            id: md5.clone(),
            title,
            author,
            format: extension,
            size,
            provider: "libgen".to_string(),
            download_id: md5,
        });
    }

    results
}

fn resolve_filename(book: &BookResult, headers: &reqwest::header::HeaderMap) -> String {
    headers
        .get(reqwest::header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            let re = Regex::new(r#"filename="?([^";\r\n]+)"?"#).ok()?;
            re.captures(v).map(|c| c[1].trim().to_string())
        })
        .unwrap_or_else(|| {
            let ext = if book.format.is_empty() {
                "bin"
            } else {
                &book.format
            };
            let safe_title: String = book
                .title
                .chars()
                .map(|c| {
                    if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            let truncated: String = safe_title.chars().take(100).collect();
            format!("{}.{}", truncated.trim(), ext)
        })
}

/// Parse the libgen ads page HTML and extract the download URL. Must be called
/// synchronously (Html is not Send).
fn extract_download_url(body: &str) -> Option<String> {
    let document = Html::parse_document(body);
    let a_sel = Selector::parse("a").unwrap();

    for a in document.select(&a_sel) {
        if let Some(href) = a.value().attr("href") {
            if href.contains("get.php") && href.contains("md5=") {
                let full_url = if href.starts_with("http") {
                    href.to_string()
                } else {
                    format!("https://libgen.li/{}", href.trim_start_matches('/'))
                };
                return Some(full_url);
            }
        }
    }
    None
}

#[async_trait]
impl BookProvider for LibgenProvider {
    fn name(&self) -> &str {
        "libgen"
    }

    async fn search(&self, query: &str) -> Result<Vec<BookResult>> {
        let encoded = urlencoding::encode(query);
        let url = format!(
            "https://libgen.li/index.php?req={}&columns%5B%5D=title&objects%5B%5D=f&objects%5B%5D=e&objects%5B%5D=s&objects%5B%5D=a&objects%5B%5D=p&objects%5B%5D=w&topics%5B%5D=l&res=25&filesuns=all",
            encoded
        );

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch LibGen search page")?;
        let body = resp
            .text()
            .await
            .context("Failed to read LibGen response body")?;
        Ok(parse_libgen_html(&body))
    }

    async fn download(&self, book: &BookResult, output_dir: &Path) -> Result<PathBuf> {
        let ads_url = format!("https://libgen.li/ads.php?md5={}", book.download_id);
        let resp = self
            .client
            .get(&ads_url)
            .send()
            .await
            .context("Failed to fetch LibGen ads page")?;
        let body = resp.text().await.context("Failed to read ads page body")?;

        let download_url =
            extract_download_url(&body).context("No download link found on LibGen ads page")?;

        let resp = self
            .client
            .get(&download_url)
            .send()
            .await
            .context("Failed to download file from LibGen")?;

        let filename = resolve_filename(book, resp.headers());
        let out_path = output_dir.join(&filename);
        let bytes = resp
            .bytes()
            .await
            .context("Failed to read download bytes")?;
        let mut file = std::fs::File::create(&out_path)
            .with_context(|| format!("Failed to create file: {}", out_path.display()))?;
        file.write_all(&bytes)
            .with_context(|| format!("Failed to write file: {}", out_path.display()))?;

        Ok(out_path)
    }

    async fn download_stream(
        &self,
        book: &BookResult,
        tx: tokio::sync::mpsc::Sender<Result<crate::types::DownloadEvent>>,
    ) {
        use crate::types::DownloadEvent;

        macro_rules! send {
            ($val:expr) => {
                if tx.send($val).await.is_err() {
                    return;
                }
            };
        }

        let ads_url = format!("https://libgen.li/ads.php?md5={}", book.download_id);
        let body = match self.client.get(&ads_url).send().await {
            Ok(r) => match r.text().await {
                Ok(t) => t,
                Err(e) => {
                    send!(Err(e.into()));
                    return;
                }
            },
            Err(e) => {
                send!(Err(e.into()));
                return;
            }
        };

        let download_url = match extract_download_url(&body) {
            Some(u) => u,
            None => {
                send!(Err(anyhow::anyhow!(
                    "No download link found on LibGen ads page"
                )));
                return;
            }
        };

        let mut resp = match self.client.get(&download_url).send().await {
            Ok(r) => r,
            Err(e) => {
                send!(Err(e.into()));
                return;
            }
        };

        let total_bytes = resp.content_length();
        let filename = resolve_filename(book, resp.headers());

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
                    send!(Ok(DownloadEvent::Data {
                        bytes: chunk.to_vec(),
                        downloaded
                    }));
                }
                Ok(None) => break,
                Err(e) => {
                    send!(Err(e.into()));
                    return;
                }
            }
        }

        send!(Ok(DownloadEvent::Done {
            filename,
            total_bytes: downloaded
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_libgen_html_typical_row() {
        let html = r#"<table>
  <tr>
    <td><a href="/ads.php?md5=abc123def456abc123def456abc123de">Book Title Here That Is Long Enough</a></td>
    <td>Author Name</td>
    <td>Publisher</td>
    <td>2023</td>
    <td>English</td>
    <td>pdf</td>
    <td>2.5 MB</td>
  </tr>
</table>"#;
        let results = parse_libgen_html(html);
        assert_eq!(results.len(), 1);
        let book = &results[0];
        assert_eq!(book.id, "abc123def456abc123def456abc123de");
        assert_eq!(book.title, "Book Title Here That Is Long Enough");
        assert_eq!(book.author, "Author Name");
        assert_eq!(book.format, "pdf");
        assert_eq!(book.size, "2.5 MB");
        assert_eq!(book.provider, "libgen");
    }

    #[test]
    fn test_parse_libgen_html_missing_md5_skipped() {
        let html = r#"<table>
  <tr>
    <td><a href="/some/other/link">Book Title That Is Long Enough Here</a></td>
    <td>Author Name</td>
    <td>pdf</td>
    <td>1 MB</td>
  </tr>
</table>"#;
        let results = parse_libgen_html(html);
        assert_eq!(results.len(), 0, "rows without md5 should be skipped");
    }

    #[test]
    fn test_parse_libgen_html_empty() {
        let results = parse_libgen_html("");
        assert_eq!(results.len(), 0);
    }
}
