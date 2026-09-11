//! Freesound.org audio search. Blocking HTTP on a worker thread.
//!
//! Pixabay was the first choice for the audio section, but Pixabay has no
//! public API for audio/music — only images and video are documented. Their
//! website's music search is not a stable, published contract, so building
//! against it would be fragile. Freesound has a real, documented public API
//! with a free key, so the audio section is backed by that instead.

use serde::Deserialize;

/// No key ships with the build — unlike Pexels, Freesound does not offer a
/// shared demo key third parties can embed. Get a free one at
/// https://freesound.org/apiv2/apply/ and set `MW_FREESOUND_KEY`.
pub fn api_key() -> String {
    std::env::var("MW_FREESOUND_KEY").unwrap_or_default().trim().to_string()
}

#[derive(Clone, Debug)]
pub struct StockAudio {
    pub id: u64,
    pub name: String,
    pub username: String,
    pub duration: f32,
    pub preview_url: String,
}

#[derive(Deserialize)]
struct FsResponse {
    results: Vec<FsSound>,
}

#[derive(Deserialize)]
struct FsSound {
    id: u64,
    name: String,
    username: String,
    duration: f32,
    previews: FsPreviews,
}

#[derive(Deserialize)]
struct FsPreviews {
    #[serde(rename = "preview-hq-mp3")]
    hq_mp3: String,
}

pub fn search(api_key: &str, query: &str, page: u32) -> Result<Vec<StockAudio>, String> {
    let url = format!(
        "https://freesound.org/apiv2/search/text/?query={}&page={}&page_size=24&fields=id,name,username,duration,previews&token={}",
        crate::stock::url_encode(query),
        page,
        api_key
    );
    let resp = ureq::get(&url).call().map_err(|e| format!("Freesound request failed: {e}"))?;
    let data: FsResponse = resp.into_json().map_err(|e| e.to_string())?;
    Ok(data
        .results
        .into_iter()
        .map(|s| StockAudio {
            id: s.id,
            name: s.name,
            username: s.username,
            duration: s.duration,
            preview_url: s.previews.hq_mp3,
        })
        .collect())
}

/// Downloads a sound's mp3 preview into the app cache, returning its local
/// path. The preview (not the original upload) is what plays fine without
/// needing the OAuth2 flow Freesound requires for original-quality files.
pub fn download(url: &str, id: u64) -> Result<String, String> {
    crate::stock::fetch_cached(url, &format!("fs-{}.mp3", id))
}
