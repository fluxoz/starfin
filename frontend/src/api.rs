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
    genre: Option<&'a str>,
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
            // Expected endpoint (you can change): GET /elements?q=...&sort=...&genre=...&only_favorites=...
            let q = ElementsQuery {
                q: query,
                sort: sort_by.as_str(),
                genre: filters.genre.as_deref(),
                only_favorites: filters.only_favorites,
            };

            let mut url = format!("{}/elements", base_url.trim_end_matches('/'));
            // Use query string (manual for simplicity)
            let mut params: Vec<(String, String)> = vec![
                ("q".into(), q.q.into()),
                ("sort".into(), q.sort.into()),
                ("only_favorites".into(), q.only_favorites.to_string()),
            ];
            if let Some(genre) = q.genre {
                params.push(("genre".into(), genre.into()));
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
        Element {
            id: "vid_1".into(),
            title: "Interstellar".into(),
            description: "A team of explorers travel through a wormhole in space in an attempt to ensure humanity's survival.".into(),
            genre: "Sci-Fi".into(),
            tags: vec!["space".into(), "drama".into(), "time".into()],
            rating: 8.7,
            year: 2014,
            duration_secs: 10140,
            director: "Christopher Nolan".into(),
        },
        Element {
            id: "vid_2".into(),
            title: "The Shawshank Redemption".into(),
            description: "Two imprisoned men bond over a number of years, finding solace and eventual redemption through acts of common decency.".into(),
            genre: "Drama".into(),
            tags: vec!["prison".into(), "hope".into(), "friendship".into()],
            rating: 9.3,
            year: 1994,
            duration_secs: 8520,
            director: "Frank Darabont".into(),
        },
        Element {
            id: "vid_3".into(),
            title: "The Dark Knight".into(),
            description: "Batman faces the Joker, a criminal mastermind who seeks to plunge Gotham City into anarchy.".into(),
            genre: "Action".into(),
            tags: vec!["superhero".into(), "crime".into(), "thriller".into()],
            rating: 9.0,
            year: 2008,
            duration_secs: 9120,
            director: "Christopher Nolan".into(),
        },
        Element {
            id: "vid_4".into(),
            title: "Pulp Fiction".into(),
            description: "The lives of two hitmen, a boxer, a gangster, and others intertwine in four tales of violence and redemption.".into(),
            genre: "Crime".into(),
            tags: vec!["crime".into(), "violence".into(), "nonlinear".into()],
            rating: 8.9,
            year: 1994,
            duration_secs: 9360,
            director: "Quentin Tarantino".into(),
        },
        Element {
            id: "vid_5".into(),
            title: "Inception".into(),
            description: "A thief who steals corporate secrets through dream-sharing technology is given the task of planting an idea.".into(),
            genre: "Sci-Fi".into(),
            tags: vec!["dreams".into(), "heist".into(), "thriller".into()],
            rating: 8.8,
            year: 2010,
            duration_secs: 8880,
            director: "Christopher Nolan".into(),
        },
    ];

    // Filter: genre
    if let Some(genre) = &filters.genre {
        data.retain(|e| e.genre.eq_ignore_ascii_case(genre));
    }

    // Filter: only favorites (stubbed; you can wire to real user prefs later)
    if filters.only_favorites {
        data.retain(|e| e.rating >= 9.0);
    }

    // Search
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

    // Sort
    match sort_by {
        SortBy::Relevance => {
            // For stub: just keep rating desc as "relevance"
            data.sort_by(|a, b| b.rating.total_cmp(&a.rating));
        }
        SortBy::NameAsc => data.sort_by(|a, b| a.title.cmp(&b.title)),
        SortBy::RatingDesc => data.sort_by(|a, b| b.rating.total_cmp(&a.rating)),
    }

    data
}
