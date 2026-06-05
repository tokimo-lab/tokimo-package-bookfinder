use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookResult {
    pub id: String,
    pub title: String,
    pub author: String,
    pub format: String,
    pub size: String,
    pub provider: String,
    pub download_id: String,
}

/// Streaming download events for binary file downloads (PDF, EPUB, etc.)
#[derive(Debug, Clone)]
pub enum DownloadEvent {
    /// First event: file metadata
    FileInfo {
        title: String,
        author: String,
        format: String,
        filename: String,
        total_bytes: Option<u64>,
    },
    /// A chunk of file data
    Data { bytes: Vec<u8>, downloaded: u64 },
    /// Download complete
    Done { filename: String, total_bytes: u64 },
}

impl std::fmt::Display for BookResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}] [{}] {} - {} ({}, {})",
            self.provider, self.format, self.title, self.author, self.size, self.id
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_book_result_display() {
        let book = BookResult {
            id: "abc123".to_string(),
            title: "Rust Programming".to_string(),
            author: "Steve Klabnik".to_string(),
            format: "pdf".to_string(),
            size: "2.5 MB".to_string(),
            provider: "libgen".to_string(),
            download_id: "abc123".to_string(),
        };
        let s = format!("{}", book);
        assert!(s.contains("libgen"), "missing provider");
        assert!(s.contains("pdf"), "missing format");
        assert!(s.contains("Rust Programming"), "missing title");
        assert!(s.contains("Steve Klabnik"), "missing author");
        assert!(s.contains("2.5 MB"), "missing size");
        assert!(s.contains("abc123"), "missing id");
    }

    #[test]
    fn test_book_result_display_empty_fields() {
        let book = BookResult {
            id: "xyz".to_string(),
            title: String::new(),
            author: String::new(),
            format: String::new(),
            size: String::new(),
            provider: "gutenberg".to_string(),
            download_id: "xyz".to_string(),
        };
        let s = format!("{}", book);
        assert!(s.contains("gutenberg"));
        assert!(s.contains("xyz"));
    }
}
