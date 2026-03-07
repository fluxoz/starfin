use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Element {
    pub id: String,
    pub title: String,
    pub description: String,
    pub genre: String,
    pub tags: Vec<String>,
    pub rating: f64,
    pub year: u16,
    pub duration_secs: u32,
    pub director: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Filters {
    pub genre: Option<String>,
    pub only_favorites: bool,
}

impl Default for Filters {
    fn default() -> Self {
        Self {
            genre: None,
            only_favorites: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum SortBy {
    Relevance,
    NameAsc,
    RatingDesc,
}

impl SortBy {
    pub fn as_str(&self) -> &'static str {
        match self {
            SortBy::Relevance => "relevance",
            SortBy::NameAsc => "name_asc",
            SortBy::RatingDesc => "rating_desc",
        }
    }
}
