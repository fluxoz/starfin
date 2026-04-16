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
    /// Selected tag values to filter by (empty = show all).  OR logic: item
    /// must have at least one of the selected tags.
    pub tag: Vec<String>,
    /// Selected actor values to filter by (empty = show all).  OR logic.
    pub actor: Vec<String>,
    /// Selected category values to filter by (empty = show all).  OR logic.
    pub category: Vec<String>,
}

impl MetadataFilter {
    /// Returns `true` if any filter is active.
    pub fn is_active(&self) -> bool {
        self.only_favorites
            || !self.tag.is_empty()
            || !self.actor.is_empty()
            || !self.category.is_empty()
    }
}
