use crate::models::{Element, Filters, SortBy};
use gloo_net::http::Request;
use serde::{Deserialize, Serialize};

#[derive(Debug)]
pub enum ApiMode {
    /// Uses the in-app stub (good for UI development)
    Stub,
    /// Uses a real HTTP endpoint
    Http { base_url: String },
}

fn api_mode() -> ApiMode {
    // Flip to Http later, e.g. ApiMode::Http { base_url: "/api".into() }
    ApiMode::Stub
}

#[derive(Clone, Debug, Serialize)]
struct ElementsQuery<'a> {
    q: &'a str,
    sort: &'a str,
    category: Option<&'a str>,
    only_favorites: bool,
}

#[derive(Clone, Debug, Deserialize)]
struct ElementsResponse {
    items: Vec<Element>,
}

pub async fn fetch_elements(query: &str, filters: &Filters, sort_by: SortBy) -> Result<Vec<Element>, String> {
    match api_mode() {
        ApiMode::Stub => Ok(stubbed_elements(query, filters, sort_by)),
        ApiMode::Http { base_url } => {
            // Expected endpoint (you can change): GET /elements?q=...&sort=...&category=...&only_favorites=...
            let q = ElementsQuery {
                q: query,
                sort: sort_by.as_str(),
                category: filters.category.as_deref(),
                only_favorites: filters.only_favorites,
            };

            let mut url = format!("{}/elements", base_url.trim_end_matches('/'));
            // Use query string (manual for simplicity)
            let mut params: Vec<(String, String)> = vec![
                ("q".into(), q.q.into()),
                ("sort".into(), q.sort.into()),
                ("only_favorites".into(), q.only_favorites.to_string()),
            ];
            if let Some(cat) = q.category {
                params.push(("category".into(), cat.into()));
            }
            let query_string = params
                .into_iter()
                .map(|(k, v)| format!("{}={}", urlencoding::encode(&k), urlencoding::encode(&v)))
                .collect::<Vec<_>>()
                .join("&");
            url = format!("{}?{}", url, query_string);

            let resp = Request::get(&url)
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
    }
}

// -------- stub data + local filtering/sorting --------

fn stubbed_elements(query: &str, filters: &Filters, sort_by: SortBy) -> Vec<Element> {
    let mut data = vec![
        Element { id: "el_1".into(), title: "Aurora".into(), subtitle: "Northern lights palette".into(), category: "Design".into(), tags: vec!["color".into(), "ui".into()], score: 92.0 },
        Element { id: "el_2".into(), title: "Borealis".into(), subtitle: "Grid component kit".into(), category: "UI".into(), tags: vec!["grid".into(), "layout".into()], score: 88.2 },
        Element { id: "el_3".into(), title: "Cinder".into(), subtitle: "Dark theme tokens".into(), category: "Design".into(), tags: vec!["theme".into(), "dark".into()], score: 95.4 },
        Element { id: "el_4".into(), title: "Delta".into(), subtitle: "API integration sample".into(), category: "Dev".into(), tags: vec!["api".into(), "http".into()], score: 81.3 },
        Element { id: "el_5".into(), title: "Ember".into(), subtitle: "Responsive cards".into(), category: "UI".into(), tags: vec!["cards".into(), "mobile".into()], score: 86.9 },
    ];

    // Filter: category
    if let Some(cat) = &filters.category {
        data.retain(|e| e.category.eq_ignore_ascii_case(cat));
    }

    // Filter: only favorites (stubbed; you can wire to real user prefs later)
    if filters.only_favorites {
        data.retain(|e| e.score >= 90.0);
    }

    // Search
    let q = query.trim().to_lowercase();
    if !q.is_empty() {
        data.retain(|e| {
            e.title.to_lowercase().contains(&q)
                || e.subtitle.to_lowercase().contains(&q)
                || e.category.to_lowercase().contains(&q)
                || e.tags.iter().any(|t| t.to_lowercase().contains(&q))
        });
    }

    // Sort
    match sort_by {
        SortBy::Relevance => {
            // For stub: just keep score desc as "relevance"
            data.sort_by(|a, b| b.score.total_cmp(&a.score));
        }
        SortBy::NameAsc => data.sort_by(|a, b| a.title.cmp(&b.title)),
        SortBy::ScoreDesc => data.sort_by(|a, b| b.score.total_cmp(&a.score)),
    }

    data
}
