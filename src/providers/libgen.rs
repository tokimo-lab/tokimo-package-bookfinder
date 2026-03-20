use super::BookProvider;
use crate::types::BookResult;
use anyhow::{Context, Result};
use regex::Regex;
use reqwest::blocking::Client;
use scraper::{Html, Selector};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub struct LibgenProvider {
    client: Client,
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

const KNOWN_EXTENSIONS: &[&str] = &["pdf", "epub", "djvu", "mobi", "chm", "azw3", "fb2", "doc", "docx", "rtf", "txt", "lit", "azw"];

impl BookProvider for LibgenProvider {
    fn name(&self) -> &str {
        "libgen"
    }

    fn search(&self, query: &str) -> Result<Vec<BookResult>> {
        let encoded = urlencoding::encode(query);
        let url = format!(
            "https://libgen.li/index.php?req={}&columns%5B%5D=title&objects%5B%5D=f&objects%5B%5D=e&objects%5B%5D=s&objects%5B%5D=a&objects%5B%5D=p&objects%5B%5D=w&topics%5B%5D=l&res=25&filesuns=all",
            encoded
        );

        let resp = self.client.get(&url).send().context("Failed to fetch LibGen search page")?;
        let body = resp.text().context("Failed to read LibGen response body")?;
        let document = Html::parse_document(&body);

        let tr_sel = Selector::parse("table tr").unwrap();
        let td_sel = Selector::parse("td").unwrap();
        let a_sel = Selector::parse("a").unwrap();
        let size_re = Regex::new(r"(\d+[\.,]?\d*)\s*(kB|KB|Kb|MB|Mb|mB|GB|Gb|gB|bytes?)").unwrap();

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
                .map(|td| td.text().collect::<Vec<_>>().join(" ").trim().to_string())
                .collect();

            if cells.is_empty() {
                continue;
            }

            // Title: first cell text longer than 20 chars
            let title = cells
                .iter()
                .find(|c| c.len() > 20)
                .cloned()
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

        Ok(results)
    }

    fn download(&self, book: &BookResult, output_dir: &Path) -> Result<PathBuf> {
        let ads_url = format!("https://libgen.li/ads.php?md5={}", book.download_id);
        let resp = self.client.get(&ads_url).send().context("Failed to fetch LibGen ads page")?;
        let body = resp.text().context("Failed to read ads page body")?;
        let document = Html::parse_document(&body);

        let a_sel = Selector::parse("a").unwrap();
        let mut download_url: Option<String> = None;

        for a in document.select(&a_sel) {
            if let Some(href) = a.value().attr("href") {
                if href.contains("get.php") && href.contains("md5=") {
                    let full_url = if href.starts_with("http") {
                        href.to_string()
                    } else {
                        format!("https://libgen.li/{}", href.trim_start_matches('/'))
                    };
                    download_url = Some(full_url);
                    break;
                }
            }
        }

        let download_url = download_url.context("No download link found on LibGen ads page")?;

        let resp = self.client.get(&download_url).send().context("Failed to download file from LibGen")?;

        // Determine filename from Content-Disposition or fallback
        let filename = resp
            .headers()
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
            });

        let out_path = output_dir.join(&filename);
        let bytes = resp.bytes().context("Failed to read download bytes")?;
        let mut file = std::fs::File::create(&out_path)
            .with_context(|| format!("Failed to create file: {}", out_path.display()))?;
        file.write_all(&bytes)
            .with_context(|| format!("Failed to write file: {}", out_path.display()))?;

        Ok(out_path)
    }
}
