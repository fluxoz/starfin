use crate::models::{Element, Filters, SortBy};
use gloo_net::http::Request;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
struct ElementsResponse {
    items: Vec<Element>,
}

/// Trigger an immediate re-scan of the media library on the server.
pub async fn trigger_scan() -> Result<(), String> {
    let resp = Request::post("/api/scan")
        .send()
        .await
        .map_err(|e| format!("Network error: {e:?}"))?;

    if !resp.ok() {
        return Err(format!("HTTP error: {}", resp.status()));
    }

    Ok(())
}

/// Fetch all videos from the API, then apply filtering and sorting locally.
pub async fn fetch_elements(
    query: &str,
    filters: &Filters,
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

    Ok(apply_filters(parsed.items, query, filters, sort_by))
}

// ── Local filtering & sorting ────────────────────────────────────────────────

fn apply_filters(
    mut data: Vec<Element>,
    query: &str,
    filters: &Filters,
    sort_by: SortBy,
) -> Vec<Element> {
    if let Some(genre) = &filters.genre {
        data.retain(|e| e.genre.eq_ignore_ascii_case(genre));
    }

    if filters.only_favorites {
        data.retain(|e| e.rating >= 9.0);
    }

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
        SortBy::Relevance => data.sort_by(|a, b| b.rating.total_cmp(&a.rating)),
        SortBy::NameAsc => data.sort_by(|a, b| a.title.cmp(&b.title)),
        SortBy::RatingDesc => data.sort_by(|a, b| b.rating.total_cmp(&a.rating)),
    }

    data
}
