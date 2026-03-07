use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Element {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub category: String,
    pub tags: Vec<String>,
    pub score: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Filters {
    pub category: Option<String>,
    pub only_favorites: bool,
}

impl Default for Filters {
    fn default() -> Self {
        Self {
            category: None,
            only_favorites: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum SortBy {
    Relevance,
    NameAsc,
    ScoreDesc,
}

impl SortBy {
    pub fn as_str(&self) -> &'static str {
        match self {
            SortBy::Relevance => "relevance",
            SortBy::NameAsc => "name_asc",
            SortBy::ScoreDesc => "score_desc",
        }
    }
}
