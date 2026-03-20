use anyhow::{Context, Result};
use regex::Regex;
use reqwest::blocking::Client;
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

impl BookProvider for GutenbergProvider {
    fn name(&self) -> &str {
        "gutenberg"
    }

    fn search(&self, query: &str) -> Result<Vec<BookResult>> {
        let url = format!(
            "https://www.gutenberg.org/ebooks/search/?query={}&submit_search=Go%21",
            urlencoding::encode(query)
        );

        let response = self
            .client
            .get(&url)
            .send()
            .context("Failed to send search request to Gutenberg")?;

        let body = response
            .text()
            .context("Failed to read Gutenberg search response")?;

        let document = Html::parse_document(&body);
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
                .find_map(|href| {
                    id_re.captures(href).map(|caps| caps[1].to_string())
                });

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

        Ok(results)
    }

    fn download(&self, book: &BookResult, output_dir: &Path) -> Result<PathBuf> {
        fs::create_dir_all(output_dir).context("Failed to create output directory")?;

        let sanitized_title: String = book
            .title
            .chars()
            .map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { '_' })
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

        let response = self.client.get(&primary_url).send();

        let response = match response {
            Ok(resp) if resp.status().is_success() => resp,
            _ => self
                .client
                .get(&fallback_url)
                .send()
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
            .context("Failed to read download response bytes")?;

        let mut file = fs::File::create(&output_path)
            .context("Failed to create output file")?;
        file.write_all(&bytes)
            .context("Failed to write book to file")?;

        Ok(output_path)
    }
}
