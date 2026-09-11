//! Pexels stock photo and video search. Blocking HTTP on a worker thread.

use serde::Deserialize;

/// The Pexels key this build ships with. Stock photos are a feature of the
/// app, not something a user should have to go and register for, so the key
/// lives here rather than behind a settings dialog. `MW_PEXELS_KEY` overrides
/// it without a rebuild.
const BUILTIN_KEY: &str = "JWmK1eGlEHADsBBPe3lFNKdEdVEgXZpbJHlBn8GT57MwwwVBAHK7MMlv";

pub fn api_key() -> String {
    match std::env::var("MW_PEXELS_KEY") {
        Ok(k) if !k.trim().is_empty() => k.trim().to_string(),
        _ => BUILTIN_KEY.trim().to_string(),
    }
}

#[derive(Clone, Debug)]
pub struct StockPhoto {
    pub id: u64,
    pub thumbnail: String,
    pub url: String,
    pub photographer: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Deserialize)]
struct PexelsResponse {
    photos: Vec<PexelsPhoto>,
}

#[derive(Deserialize)]
struct PexelsPhoto {
    id: u64,
    width: u32,
    height: u32,
    photographer: String,
    src: PexelsSrc,
}

#[derive(Deserialize)]
struct PexelsSrc {
    large: String,
    medium: String,
}

#[derive(Clone, Debug)]
pub struct StockVideo {
    pub id: u64,
    pub thumbnail: String,
    pub url: String,
    pub photographer: String,
    pub width: u32,
    pub height: u32,
    pub duration: f32,
}

#[derive(Deserialize)]
struct PexelsVideoResponse {
    videos: Vec<PexelsVideo>,
}

#[derive(Deserialize)]
struct PexelsVideo {
    id: u64,
    width: u32,
    height: u32,
    duration: f32,
    image: String,
    user: PexelsUser,
    video_files: Vec<PexelsVideoFile>,
}

#[derive(Deserialize)]
struct PexelsUser {
    name: String,
}

#[derive(Deserialize)]
struct PexelsVideoFile {
    // Pexels returns `null` for some renditions (audio-only tracks, etc.),
    // not just an absent field — these have to stay optional.
    quality: Option<String>,
    width: Option<u32>,
    link: String,
}

/// The smallest "sd" rendition under 960px wide, or failing that whatever
/// file comes first — full "hd"/4k originals cost far more to download than
/// a library preview needs.
fn pick_video_file(files: &[PexelsVideoFile]) -> Option<&PexelsVideoFile> {
    files
        .iter()
        .filter(|f| f.quality.as_deref() == Some("sd") && f.width.unwrap_or(0) <= 960)
        .max_by_key(|f| f.width.unwrap_or(0))
        .or_else(|| files.iter().min_by_key(|f| f.width.unwrap_or(u32::MAX)))
}

pub fn search_videos(api_key: &str, query: &str, page: u32) -> Result<Vec<StockVideo>, String> {
    let url = format!(
        "https://api.pexels.com/videos/search?query={}&per_page=24&page={}",
        url_encode(query),
        page
    );
    let resp = ureq::get(&url)
        .set("Authorization", api_key)
        .call()
        .map_err(|e| format!("Pexels request failed: {e}"))?;
    let data: PexelsVideoResponse = resp.into_json().map_err(|e| e.to_string())?;
    Ok(data
        .videos
        .into_iter()
        .filter_map(|v| {
            let file = pick_video_file(&v.video_files)?;
            Some(StockVideo {
                id: v.id,
                thumbnail: v.image,
                url: file.link.clone(),
                photographer: v.user.name,
                width: v.width,
                height: v.height,
                duration: v.duration,
            })
        })
        .collect())
}

/// Downloads a video into the app cache, returning its local path.
pub fn download_video(url: &str, id: u64) -> Result<String, String> {
    fetch_cached(url, &format!("{}.mp4", id))
}

pub(crate) fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

pub fn search(api_key: &str, query: &str, page: u32) -> Result<Vec<StockPhoto>, String> {
    let url = format!(
        "https://api.pexels.com/v1/search?query={}&per_page=24&page={}",
        url_encode(query),
        page
    );
    let resp = ureq::get(&url)
        .set("Authorization", api_key)
        .call()
        .map_err(|e| format!("Pexels request failed: {e}"))?;
    let data: PexelsResponse = resp.into_json().map_err(|e| e.to_string())?;
    Ok(data
        .photos
        .into_iter()
        .map(|p| StockPhoto {
            id: p.id,
            thumbnail: p.src.medium,
            url: p.src.large,
            photographer: p.photographer,
            width: p.width,
            height: p.height,
        })
        .collect())
}

/// Downloads a photo into the app cache, returning its local path. Cached
/// downloads are reused, so dragging the same photo twice costs one request.
pub fn download(url: &str, id: u64) -> Result<String, String> {
    fetch_cached(url, &format!("{}.jpg", id))
}

/// The smaller preview image shown in the library grid.
pub fn download_thumb(url: &str, id: u64) -> Result<String, String> {
    fetch_cached(url, &format!("{}-thumb.jpg", id))
}

pub(crate) fn fetch_cached(url: &str, name: &str) -> Result<String, String> {
    let dir = crate::projects::cache_dir().join("stock");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(name);
    if path.exists() {
        return Ok(path.to_string_lossy().to_string());
    }
    let resp = ureq::get(url).call().map_err(|e| format!("Download failed: {e}"))?;
    let mut bytes = Vec::new();
    std::io::copy(&mut resp.into_reader(), &mut bytes).map_err(|e| e.to_string())?;
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().to_string())
}
