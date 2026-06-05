use anyhow::{Context, Result};
use async_trait::async_trait;
use regex::Regex;
use reqwest::Client;
use scraper::{Html, Selector};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::BookProvider;
use crate::types::BookResult;

pub struct GutenbergProvider {
    client: Client,
}

impl Default for GutenbergProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl GutenbergProvider {
    pub fn new() -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .build()
            .expect("Failed to build HTTP client");
        Self { client }
    }
}

pub fn parse_gutenberg_html(html: &str) -> Vec<BookResult> {
    let document = Html::parse_document(html);
    let booklink_sel = Selector::parse("li.booklink").unwrap();
    let title_sel = Selector::parse("span.title").unwrap();
    let subtitle_sel = Selector::parse("span.subtitle").unwrap();
    let link_sel = Selector::parse("a").unwrap();
    let id_re = Regex::new(r"/ebooks/(\d+)").unwrap();

    let mut results = Vec::new();

    for item in document.select(&booklink_sel) {
        let title = item
            .select(&title_sel)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
            .unwrap_or_default();

        if title.is_empty() {
            continue;
        }

        let author = item
            .select(&subtitle_sel)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
            .unwrap_or_else(|| "Unknown".to_string());

        let book_id = item
            .select(&link_sel)
            .filter_map(|a| a.value().attr("href"))
            .find_map(|href| id_re.captures(href).map(|caps| caps[1].to_string()));

        let book_id = match book_id {
            Some(id) => id,
            None => continue,
        };

        results.push(BookResult {
            id: book_id.clone(),
            title,
            author,
            format: "epub".to_string(),
            size: "~1 MB".to_string(),
            provider: "gutenberg".to_string(),
            download_id: book_id,
        });
    }

    results
}

#[async_trait]
impl BookProvider for GutenbergProvider {
    fn name(&self) -> &str {
        "gutenberg"
    }

    async fn search(&self, query: &str) -> Result<Vec<BookResult>> {
        let url = format!(
            "https://www.gutenberg.org/ebooks/search/?query={}&submit_search=Go%21",
            urlencoding::encode(query)
        );

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to send search request to Gutenberg")?;
        let body = response
            .text()
            .await
            .context("Failed to read Gutenberg search response")?;

        Ok(parse_gutenberg_html(&body))
    }

    async fn download(&self, book: &BookResult, output_dir: &Path) -> Result<PathBuf> {
        fs::create_dir_all(output_dir).context("Failed to create output directory")?;

        let sanitized_title: String = book
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
        let filename = format!("{}.epub", sanitized_title.trim());
        let output_path = output_dir.join(&filename);

        let primary_url = format!(
            "https://www.gutenberg.org/ebooks/{}.epub3.images",
            book.download_id
        );
        let fallback_url = format!(
            "https://www.gutenberg.org/ebooks/{}.epub.images",
            book.download_id
        );

        let response = self.client.get(&primary_url).send().await;

        let response = match response {
            Ok(resp) if resp.status().is_success() => resp,
            _ => self
                .client
                .get(&fallback_url)
                .send()
                .await
                .context("Failed to download from both Gutenberg URLs")?,
        };

        if !response.status().is_success() {
            anyhow::bail!(
                "Download failed with status {} for book {}",
                response.status(),
                book.download_id
            );
        }

        let bytes = response
            .bytes()
            .await
            .context("Failed to read download response bytes")?;

        let mut file = fs::File::create(&output_path).context("Failed to create output file")?;
        file.write_all(&bytes)
            .context("Failed to write book to file")?;

        Ok(output_path)
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

        let sanitized_title: String = book
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
        let filename = format!("{}.epub", sanitized_title.trim());

        let primary_url = format!(
            "https://www.gutenberg.org/ebooks/{}.epub3.images",
            book.download_id
        );
        let fallback_url = format!(
            "https://www.gutenberg.org/ebooks/{}.epub.images",
            book.download_id
        );

        let response = self.client.get(&primary_url).send().await;
        let mut resp = match response {
            Ok(r) if r.status().is_success() => r,
            _ => match self.client.get(&fallback_url).send().await {
                Ok(r) if r.status().is_success() => r,
                Ok(r) => {
                    send!(Err(anyhow::anyhow!(
                        "Download failed with status {} for book {}",
                        r.status(),
                        book.download_id
                    )));
                    return;
                }
                Err(e) => {
                    send!(Err(e.into()));
                    return;
                }
            },
        };

        let total_bytes = resp.content_length();

        send!(Ok(DownloadEvent::FileInfo {
            title: book.title.clone(),
            author: book.author.clone(),
            format: "epub".to_string(),
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
    fn test_parse_gutenberg_html_typical() {
        let html = r#"<ul class="results">
  <li class="booklink">
    <a href="/ebooks/2701">
      <span class="title">Moby Dick</span>
      <span class="subtitle">Melville, Herman</span>
    </a>
  </li>
</ul>"#;
        let results = parse_gutenberg_html(html);
        assert_eq!(results.len(), 1);
        let book = &results[0];
        assert_eq!(book.id, "2701");
        assert_eq!(book.title, "Moby Dick");
        assert_eq!(book.author, "Melville, Herman");
        assert_eq!(book.format, "epub");
        assert_eq!(book.provider, "gutenberg");
    }

    #[test]
    fn test_parse_gutenberg_html_no_id_skipped() {
        let html = r#"<ul class="results">
  <li class="booklink">
    <a href="/no-ebook-id">
      <span class="title">Some Book Without ID</span>
    </a>
  </li>
</ul>"#;
        let results = parse_gutenberg_html(html);
        assert_eq!(
            results.len(),
            0,
            "entries without ebook id should be skipped"
        );
    }

    #[test]
    fn test_parse_gutenberg_html_empty_title_skipped() {
        let html = r#"<ul class="results">
  <li class="booklink">
    <a href="/ebooks/123"><span class="title"></span></a>
  </li>
</ul>"#;
        let results = parse_gutenberg_html(html);
        assert_eq!(
            results.len(),
            0,
            "entries with empty title should be skipped"
        );
    }

    #[test]
    fn test_parse_gutenberg_html_empty() {
        let results = parse_gutenberg_html("");
        assert_eq!(results.len(), 0);
    }
}
