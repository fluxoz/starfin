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

/// Thumbnail progress received over the `/api/progress/ws` WebSocket.
#[derive(Clone, Debug, Deserialize)]
pub struct ThumbProgressMsg {
    pub current: u32,
    pub total: u32,
    pub active: bool,
    pub phase: String,
    /// The video ID currently being thumbnailed, if any.
    pub current_id: Option<String>,
}

/// Sprite progress received over the `/api/progress/ws` WebSocket.
#[derive(Clone, Debug, Deserialize)]
pub struct SpriteProgressMsg {
    pub current: u32,
    pub total: u32,
    pub active: bool,
    /// The video ID currently getting a sprite sheet, if any.
    pub current_id: Option<String>,
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
