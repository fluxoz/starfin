use crate::models::{Element, SortBy};
use gloo_net::http::Request;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
struct ElementsResponse {
    items: Vec<Element>,
}

/// Progress frame received over the scan WebSocket: `{"current":N,"total":M}`.
#[derive(Clone, Debug, Deserialize)]
pub struct ScanProgressData {
    pub current: u32,
    pub total: u32,
}

/// Fetch all videos from the API, then apply filtering and sorting locally.
pub async fn fetch_elements(
    query: &str,
    sort_by: SortBy,
) -> Result<Vec<Element>, String> {
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

    Ok(apply_filters(parsed.items, query, sort_by))
}

// ── Local filtering & sorting ────────────────────────────────────────────────

fn apply_filters(
    mut data: Vec<Element>,
    query: &str,
    sort_by: SortBy,
) -> Vec<Element> {
    let q = query.trim().to_lowercase();
    if !q.is_empty() {
        data.retain(|e| {
            e.title.to_lowercase().contains(&q)
                || e.description.to_lowercase().contains(&q)
                || e.genre.to_lowercase().contains(&q)
                || e.director.to_lowercase().contains(&q)
                || e.tags.iter().any(|t| t.to_lowercase().contains(&q))
        });
    }

    match sort_by {
        SortBy::DateAddedNewest => data.sort_by(|a, b| b.date_added.cmp(&a.date_added)),
        SortBy::DateAddedOldest => data.sort_by(|a, b| a.date_added.cmp(&b.date_added)),
        SortBy::NameAsc => data.sort_by(|a, b| a.title.cmp(&b.title)),
    }

    data
}
