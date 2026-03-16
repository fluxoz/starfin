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
    /// Whether the user has favorited this media file.
    #[serde(default)]
    pub favorite: bool,
    /// User-defined list of actors / people appearing in the media.
    #[serde(default)]
    pub actors: Vec<String>,
    /// User-defined genre / category labels.
    #[serde(default)]
    pub categories: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum SortBy {
    DateAddedNewest,
    DateAddedOldest,
    NameAsc,
    NameDesc,
    RatingHighest,
    FavoritesFirst,
    YearNewest,
    YearOldest,
}

impl SortBy {
    pub fn as_str(&self) -> &'static str {
        match self {
            SortBy::DateAddedNewest => "date_newest",
            SortBy::DateAddedOldest => "date_oldest",
            SortBy::NameAsc => "name_asc",
            SortBy::NameDesc => "name_desc",
            SortBy::RatingHighest => "rating_highest",
            SortBy::FavoritesFirst => "favorites_first",
            SortBy::YearNewest => "year_newest",
            SortBy::YearOldest => "year_oldest",
        }
    }
}

/// Additional filters based on user-defined metadata fields.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MetadataFilter {
    /// Show only favorited items.
    pub only_favorites: bool,
    /// Minimum star rating (0 = disabled, 1–5 = minimum stars required).
    pub min_rating: u8,
    /// Substring to match against tags (empty = disabled).
    pub tag: String,
    /// Substring to match against actors (empty = disabled).
    pub actor: String,
    /// Substring to match against categories (empty = disabled).
    pub category: String,
}

impl MetadataFilter {
    /// Returns `true` if any filter is active.
    pub fn is_active(&self) -> bool {
        self.only_favorites
            || self.min_rating > 0
            || !self.tag.is_empty()
            || !self.actor.is_empty()
            || !self.category.is_empty()
    }
}
