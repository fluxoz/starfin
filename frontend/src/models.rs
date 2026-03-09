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
    pub date_added: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum SortBy {
    DateAddedNewest,
    DateAddedOldest,
    NameAsc,
}

impl SortBy {
    pub fn as_str(&self) -> &'static str {
        match self {
            SortBy::DateAddedNewest => "date_newest",
            SortBy::DateAddedOldest => "date_oldest",
            SortBy::NameAsc => "name_asc",
        }
    }
}
