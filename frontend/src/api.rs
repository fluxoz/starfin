use crate::models::{Element, MetadataFilter, SortBy};
use gloo_net::http::Request;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
struct ElementsResponse {
    items: Vec<Element>,
}

/// Progress frame received over the scan WebSocket.
/// Each message after the initial `{"current":0,"total":N}` frame also carries
/// the newly-scanned `item` so the frontend can stream cards into the grid.
#[derive(Clone, Debug, Deserialize)]
pub struct ScanProgressData {
    pub current: u32,
    pub total: u32,
    pub item: Option<Element>,
}

/// Thumbnail progress received over the `/api/progress/ws` WebSocket.
#[derive(Clone, Debug, Deserialize)]
pub struct ThumbProgressMsg {
    pub current: u32,
    pub total: u32,
    pub active: bool,
    pub phase: String,
    /// The video IDs currently being thumbnailed (may be multiple when running
    /// in parallel).  Defaults to an empty list so that messages from an older
    /// backend that omits this field still deserialize successfully.
    #[serde(default)]
    pub current_ids: Vec<String>,
}

/// Sprite progress received over the `/api/progress/ws` WebSocket.
#[derive(Clone, Debug, Deserialize)]
pub struct SpriteProgressMsg {
    pub current: u32,
    pub total: u32,
    pub active: bool,
    /// The video IDs currently getting sprite sheets (may be multiple when
    /// running in parallel).  Defaults to an empty list so that messages from
    /// an older backend that omits this field still deserialize successfully.
    #[serde(default)]
    pub current_ids: Vec<String>,
}

/// Pre-cache progress received over the `/api/progress/ws` WebSocket.
#[derive(Clone, Debug, Deserialize)]
pub struct PrecacheProgressMsg {
    pub current: u32,
    pub total: u32,
    pub active: bool,
    /// The video ID currently being pre-cached, if any.
    pub current_id: Option<String>,
}

/// Combined progress update sent by `GET /api/progress/ws` every 500 ms.
#[derive(Clone, Debug, Deserialize)]
pub struct ProgressUpdate {
    pub thumb: ThumbProgressMsg,
    pub sprite: SpriteProgressMsg,
    pub precache: PrecacheProgressMsg,
}

/// Hardware acceleration info returned by `GET /api/hwaccel`.
#[derive(Clone, Debug, Deserialize)]
pub struct HwAccelInfo {
    pub label: String,
    pub encoder: String,
}

// ── Authentication API ───────────────────────────────────────────────────────

/// Response from `GET /api/auth/status`.
#[derive(Clone, Debug, Deserialize)]
pub struct AuthStatus {
    pub password_protection: bool,
    pub password_set: bool,
    pub authenticated: bool,
}

/// Fetch the current authentication status.
pub async fn fetch_auth_status() -> Result<AuthStatus, String> {
    let resp = Request::get("/api/auth/status")
        .send()
        .await
        .map_err(|e| format!("Network error: {e:?}"))?;

    if !resp.ok() {
        return Err(format!("HTTP error: {}", resp.status()));
    }

    resp.json()
        .await
        .map_err(|e| format!("Invalid JSON: {e:?}"))
}

/// Set the initial password (when none is set yet).
pub async fn set_password(password: &str, confirm: &str) -> Result<(), String> {
    let body = serde_json::json!({
        "password": password,
        "confirm": confirm,
    });

    let resp = Request::post("/api/auth/set-password")
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .map_err(|e| format!("Request error: {e:?}"))?
        .send()
        .await
        .map_err(|e| format!("Network error: {e:?}"))?;

    if !resp.ok() {
        #[derive(Deserialize)]
        struct ErrResp { error: String }
        if let Ok(err) = resp.json::<ErrResp>().await {
            return Err(err.error);
        }
        return Err(format!("HTTP error: {}", resp.status()));
    }

    Ok(())
}

/// Login with a password.
pub async fn login(password: &str) -> Result<(), String> {
    let body = serde_json::json!({ "password": password });

    let resp = Request::post("/api/auth/login")
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .map_err(|e| format!("Request error: {e:?}"))?
        .send()
        .await
        .map_err(|e| format!("Network error: {e:?}"))?;

    if !resp.ok() {
        #[derive(Deserialize)]
        struct ErrResp { error: String }
        if let Ok(err) = resp.json::<ErrResp>().await {
            return Err(err.error);
        }
        return Err(format!("HTTP error: {}", resp.status()));
    }

    Ok(())
}

/// Fetch the detected hardware acceleration backend from `/api/hwaccel`.
pub async fn fetch_hwaccel() -> Result<HwAccelInfo, String> {
    let resp = Request::get("/api/hwaccel")
        .send()
        .await
        .map_err(|e| format!("Network error: {e:?}"))?;

    if !resp.ok() {
        return Err(format!("HTTP error: {}", resp.status()));
    }

    resp.json()
        .await
        .map_err(|e| format!("Invalid JSON: {e:?}"))
}

/// Fetch the raw (unfiltered) video list from the API.
pub async fn fetch_all_videos() -> Result<Vec<Element>, String> {
    let resp = Request::get("/api/videos")
        .send()
        .await
        .map_err(|e| format!("Network error: {e:?}"))?;

    if !resp.ok() {
        return Err(format!("HTTP error: {}", resp.status()));
    }

    let parsed: ElementsResponse = resp
        .json()
        .await
        .map_err(|e| format!("Invalid JSON: {e:?}"))?;

    Ok(parsed.items)
}

// ── Local filtering & sorting ────────────────────────────────────────────────

pub fn apply_filters(
    data: &[Element],
    query: &str,
    sort_by: SortBy,
    meta_filter: &MetadataFilter,
) -> Vec<Element> {
    let q = query.trim().to_lowercase();

    let mut result: Vec<Element> = data
        .iter()
        .filter(|e| {
            // Text search across all fields including user-defined metadata.
            if !q.is_empty() {
                let matches = e.title.to_lowercase().contains(&q)
                    || e.description.to_lowercase().contains(&q)
                    || e.genre.to_lowercase().contains(&q)
                    || e.director.to_lowercase().contains(&q)
                    || e.tags.iter().any(|t| t.to_lowercase().contains(&q))
                    || e.actors.iter().any(|a| a.to_lowercase().contains(&q))
                    || e.categories.iter().any(|c| c.to_lowercase().contains(&q));
                if !matches {
                    return false;
                }
            }
            // Favorites-only filter.
            if meta_filter.only_favorites && !e.favorite {
                return false;
            }
            // Minimum rating filter.
            if meta_filter.min_rating > 0 && (e.rating.round() as u8) < meta_filter.min_rating {
                return false;
            }
            // Tag multi-select filter (OR): item must have at least one selected tag.
            if !meta_filter.tag.is_empty()
                && !meta_filter.tag.iter().any(|sel| e.tags.contains(sel))
            {
                return false;
            }
            // Actor multi-select filter (OR): item must have at least one selected actor.
            if !meta_filter.actor.is_empty()
                && !meta_filter.actor.iter().any(|sel| e.actors.contains(sel))
            {
                return false;
            }
            // Category multi-select filter (OR): item must have at least one selected category.
            if !meta_filter.category.is_empty()
                && !meta_filter.category.iter().any(|sel| e.categories.contains(sel))
            {
                return false;
            }
            true
        })
        .cloned()
        .collect();

    match sort_by {
        SortBy::DateAddedNewest => result.sort_by(|a, b| b.date_added.cmp(&a.date_added)),
        SortBy::DateAddedOldest => result.sort_by(|a, b| a.date_added.cmp(&b.date_added)),
        SortBy::NameAsc => result.sort_by(|a, b| a.title.cmp(&b.title)),
        SortBy::NameDesc => result.sort_by(|a, b| b.title.cmp(&a.title)),
        SortBy::RatingHighest => result.sort_by(|a, b| {
            b.rating
                .partial_cmp(&a.rating)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        SortBy::FavoritesFirst => result.sort_by(|a, b| {
            b.favorite
                .cmp(&a.favorite)
                .then_with(|| b.date_added.cmp(&a.date_added))
        }),
        SortBy::YearNewest => result.sort_by(|a, b| b.year.cmp(&a.year)),
        SortBy::YearOldest => result.sort_by(|a, b| a.year.cmp(&b.year)),
    }

    result
}

/// Update user-defined metadata for a video via `PATCH /api/videos/{id}/metadata`.
pub async fn update_metadata(
    video_id: &str,
    favorite: Option<bool>,
    rating: Option<f64>,
    tags: Option<Vec<String>>,
    actors: Option<Vec<String>>,
    categories: Option<Vec<String>>,
) -> Result<Element, String> {
    let mut body = serde_json::Map::new();
    if let Some(fav) = favorite {
        body.insert("favorite".into(), serde_json::Value::Bool(fav));
    }
    if let Some(r) = rating {
        body.insert("rating".into(), serde_json::json!(r));
    }
    if let Some(t) = tags {
        body.insert("tags".into(), serde_json::json!(t));
    }
    if let Some(a) = actors {
        body.insert("actors".into(), serde_json::json!(a));
    }
    if let Some(c) = categories {
        body.insert("categories".into(), serde_json::json!(c));
    }

    let resp = Request::patch(&format!("/api/videos/{video_id}/metadata"))
        .header("Content-Type", "application/json")
        .body(serde_json::Value::Object(body).to_string())
        .map_err(|e| format!("Request error: {e:?}"))?
        .send()
        .await
        .map_err(|e| format!("Network error: {e:?}"))?;

    if !resp.ok() {
        return Err(format!("HTTP error: {}", resp.status()));
    }

    resp.json()
        .await
        .map_err(|e| format!("Invalid JSON: {e:?}"))
}
