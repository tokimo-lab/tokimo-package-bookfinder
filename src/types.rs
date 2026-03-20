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

impl std::fmt::Display for BookResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] [{}] {} - {} ({}, {})", 
            self.provider, self.format, self.title, self.author, self.size, self.id)
    }
}
