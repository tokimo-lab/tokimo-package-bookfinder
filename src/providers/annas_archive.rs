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

pub struct AnnasArchiveProvider {
    client: Client,
    lang: String,
}

impl AnnasArchiveProvider {
    pub fn new(lang: impl Into<String>) -> Self {
        let client = Client::builder()
            .danger_accept_invalid_certs(true)
            .timeout(Duration::from_secs(60))
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .build()
            .expect("Failed to build HTTP client");
        Self { client, lang: lang.into() }
    }
}

pub fn parse_annas_html(html: &str, _lang: &str) -> Vec<BookResult> {
    let document = Html::parse_document(html);

    let a_sel = Selector::parse("a[href]").unwrap();
    let md5_re = Regex::new(r"^/md5/([a-f0-9]{32})$").unwrap();
    let size_re = Regex::new(r"^\d[\d.,]*\s*(kB|KB|MB|GB|bytes?)$").unwrap();
    let ext_re = Regex::new(r"^(?i)(pdf|epub|djvu|mobi|chm|azw3?|fb2|docx?|rtf|txt|lit|zip|rar)$").unwrap();

    let mut seen = std::collections::HashSet::new();
    let mut results = Vec::new();

    for a in document.select(&a_sel) {
        let href = match a.value().attr("href") {
            Some(h) => h,
            None => continue,
        };

        let caps = match md5_re.captures(href) {
            Some(c) => c,
            None => continue,
        };
        let md5 = caps[1].to_string();

        if !seen.insert(md5.clone()) {
            continue;
        }

        // Collect all text lines from the link element and its descendants
        let raw_text: String = a.text().collect::<Vec<_>>().join("\n");
        let lines: Vec<&str> = raw_text
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect();

        // Heuristic: first non-trivial line is the title
        let title = lines
            .iter()
            .find(|l| l.len() > 5)
            .map(|s| s.chars().take(150).collect::<String>())
            .unwrap_or_else(|| md5.clone());

        // Look for a line that looks like author (before the metadata line)
        // Metadata line typically contains "|" separators: "2023 | English | PDF | 2.3 MB"
        let meta_line_idx = lines.iter().position(|l| l.contains(" | ") || l.contains('|'));

        let author = meta_line_idx
            .and_then(|idx| if idx > 1 { lines.get(idx - 1) } else { None })
            .unwrap_or_else(|| lines.get(1).unwrap_or(&""))
            .to_string();

        // Parse metadata from the pipe-separated line
        let (mut format, mut size, mut language) = (String::new(), String::new(), String::new());
        if let Some(idx) = meta_line_idx {
            let parts: Vec<&str> = lines[idx].split('|').map(|s| s.trim()).collect();
            for part in &parts {
                if size_re.is_match(part) {
                    size = part.to_string();
                } else if ext_re.is_match(part) {
                    format = part.to_lowercase();
                } else if part.len() >= 2
                    && part.chars().all(|c| c.is_ascii_alphabetic() || c == '-')
                    && language.is_empty()
                {
                    language = part.to_string();
                }
            }
        }

        results.push(BookResult {
            id: md5.clone(),
            title,
            author,
            format,
            size,
            provider: format!("annas-archive{}", if language.is_empty() { String::new() } else { format!(" ({})", language) }),
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
            let ext = if book.format.is_empty() { "bin" } else { &book.format };
            let safe_title: String = book
                .title
                .chars()
                .map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { '_' })
                .collect();
            let truncated: String = safe_title.chars().take(100).collect();
            format!("{}.{}", truncated.trim(), ext)
        })
}

/// Extract download URL from libgen ads page HTML. Sync because Html is not Send.
fn extract_libgen_download_url(body: &str) -> Option<String> {
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

/// Extract fallback download URL from Anna's Archive detail page. Sync because Html is not Send.
fn extract_annas_detail_download_url(body: &str) -> Option<String> {
    let document = Html::parse_document(body);
    let a_sel = Selector::parse("a").unwrap();

    for a in document.select(&a_sel) {
        if let Some(href) = a.value().attr("href") {
            let text = a.text().collect::<String>().to_lowercase();
            let is_fast = text.contains("fast") || text.contains("download");
            let is_direct = href.contains("ipfs")
                || href.contains("libgen")
                || href.ends_with(".pdf")
                || href.ends_with(".epub");
            if (is_fast || is_direct) && href.starts_with("http") {
                return Some(href.to_string());
            }
        }
    }
    None
}

/// Resolve the actual download URL for an Anna's Archive book.
async fn resolve_download_url(client: &Client, book: &BookResult) -> Result<String> {
    // First try the libgen.li ads page
    let ads_url = format!("https://libgen.li/ads.php?md5={}", book.download_id);
    let resp = client.get(&ads_url).send().await
        .context("Failed to fetch LibGen ads page for Anna's Archive MD5")?;
    let body = resp.text().await.context("Failed to read ads page body")?;

    if let Some(url) = extract_libgen_download_url(&body) {
        return Ok(url);
    }

    // Fall back to Anna's Archive detail page
    let aa_url = format!("https://annas-archive.org/md5/{}", book.download_id);
    let resp2 = client.get(&aa_url).send().await
        .context("Failed to fetch Anna's Archive MD5 detail page")?;
    let body2 = resp2.text().await
        .context("Failed to read Anna's Archive detail page")?;

    extract_annas_detail_download_url(&body2)
        .context("No download link found for this book")
}

#[async_trait]
impl BookProvider for AnnasArchiveProvider {
    fn name(&self) -> &str {
        "annas-archive"
    }

    async fn search(&self, query: &str) -> Result<Vec<BookResult>> {
        let encoded = urlencoding::encode(query);
        let url = format!(
            "https://annas-archive.org/search?q={}&lang={}&ext=",
            encoded, self.lang
        );

        let resp = self.client.get(&url).send().await
            .context("Failed to fetch Anna's Archive search page")?;
        let body = resp.text().await
            .context("Failed to read Anna's Archive response body")?;
        Ok(parse_annas_html(&body, &self.lang))
    }

    async fn download(&self, book: &BookResult, output_dir: &Path) -> Result<PathBuf> {
        let download_url = resolve_download_url(&self.client, book).await?;

        let resp = self.client.get(&download_url).send().await
            .context("Failed to download file")?;

        let filename = resolve_filename(book, resp.headers());
        let out_path = output_dir.join(&filename);
        let bytes = resp.bytes().await.context("Failed to read download bytes")?;
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
            ($val:expr) => { if tx.send($val).await.is_err() { return; } };
        }

        let download_url = match resolve_download_url(&self.client, book).await {
            Ok(u) => u,
            Err(e) => { send!(Err(e)); return; }
        };

        let mut resp = match self.client.get(&download_url).send().await {
            Ok(r) => r,
            Err(e) => { send!(Err(e.into())); return; }
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
                    send!(Ok(DownloadEvent::Data { bytes: chunk.to_vec(), downloaded }));
                }
                Ok(None) => break,
                Err(e) => { send!(Err(e.into())); return; }
            }
        }

        send!(Ok(DownloadEvent::Done { filename, total_bytes: downloaded }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_annas_html_chinese_title() {
        let html = r#"<html><body>
  <a href="/md5/aabbccddeeff00112233445566778899">
    三体
    刘慈欣
    2008 | Chinese | epub | 1.2 MB
  </a>
</body></html>"#;
        let results = parse_annas_html(html, "zh");
        assert_eq!(results.len(), 1);
        let book = &results[0];
        assert_eq!(book.id, "aabbccddeeff00112233445566778899");
        assert!(book.title.contains("三体"), "title should contain 三体, got: {}", book.title);
        assert_eq!(book.format, "epub");
        assert!(book.provider.contains("annas-archive"));
    }

    #[test]
    fn test_parse_annas_html_deduplication() {
        let html = r#"<html><body>
  <a href="/md5/aabbccddeeff00112233445566778899">First Title Long Enough Here</a>
  <a href="/md5/aabbccddeeff00112233445566778899">Duplicate Link</a>
</body></html>"#;
        let results = parse_annas_html(html, "en");
        assert_eq!(results.len(), 1, "duplicate md5 links should be deduplicated");
    }

    #[test]
    fn test_parse_annas_html_valid_entry() {
        let html = r#"<html><body>
  <a href="/md5/00112233445566778899aabbccddeeff">
    The Great Rust Book
    John Doe
    2022 | English | pdf | 3.4 MB
  </a>
</body></html>"#;
        let results = parse_annas_html(html, "en");
        assert_eq!(results.len(), 1);
        let book = &results[0];
        assert_eq!(book.id, "00112233445566778899aabbccddeeff");
        assert!(book.title.contains("Rust Book"), "got: {}", book.title);
        assert_eq!(book.format, "pdf");
        assert_eq!(book.size, "3.4 MB");
    }

    #[test]
    fn test_parse_annas_html_empty() {
        let results = parse_annas_html("", "en");
        assert_eq!(results.len(), 0);
    }
}
