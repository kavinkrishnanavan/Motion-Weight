//! Project storage: one JSON file per project plus a small index, under the
//! user's roaming app data directory.

use crate::model::{Project, ProjectMeta};
use std::path::PathBuf;

pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir());
    let dir = base.join("MotionWeight");
    let _ = std::fs::create_dir_all(dir.join("projects"));
    dir
}

pub fn cache_dir() -> PathBuf {
    let dir = data_dir().join("cache");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn index_path() -> PathBuf {
    data_dir().join("index.json")
}

fn project_path(id: &str) -> PathBuf {
    data_dir().join("projects").join(format!("{}.json", id))
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn read_index() -> Vec<ProjectMeta> {
    std::fs::read_to_string(index_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_index(list: &[ProjectMeta]) {
    if let Ok(text) = serde_json::to_string_pretty(list) {
        let _ = std::fs::write(index_path(), text);
    }
}

/// Newest first.
pub fn list() -> Vec<ProjectMeta> {
    let mut list = read_index();
    list.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    list
}

/// Ids only have to be unique on this machine, and the clock plus a counter
/// gives that without pulling in a uuid crate.
fn fresh_id() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    format!("{:x}-{:x}", now_ms(), SEQ.fetch_add(1, Ordering::Relaxed))
}

pub fn create(name: &str) -> ProjectMeta {
    let name = name.trim();
    let meta = ProjectMeta {
        id: fresh_id(),
        name: if name.is_empty() { "Untitled Project".to_string() } else { name.to_string() },
        created_at: now_ms(),
        updated_at: now_ms(),
    };
    let mut list = read_index();
    list.push(meta.clone());
    write_index(&list);
    save(&meta.id, &Project::default());
    meta
}

pub fn load(id: &str) -> Project {
    std::fs::read_to_string(project_path(id))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(id: &str, project: &Project) {
    if let Ok(text) = serde_json::to_string(project) {
        let _ = std::fs::write(project_path(id), text);
    }
    let mut list = read_index();
    if let Some(meta) = list.iter_mut().find(|m| m.id == id) {
        meta.updated_at = now_ms();
        write_index(&list);
    }
}

/// Autosave, off the UI thread.
///
/// `save` serializes the whole project and rewrites two files. Called straight
/// from the frame loop — which is what the 400 ms autosave did — that lands as
/// a stall in the middle of whatever drag is in progress, every 400 ms, for as
/// long as the user keeps editing. One dedicated writer thread keeps the
/// ordering guarantee (never two writers racing on the same file) while
/// costing the frame only a `Project` clone.
pub fn save_async(id: String, project: Project) {
    use std::sync::mpsc::{channel, Sender};
    use std::sync::OnceLock;
    static SAVER: OnceLock<Sender<(String, Project)>> = OnceLock::new();
    let tx = SAVER.get_or_init(|| {
        let (tx, rx) = channel::<(String, Project)>();
        std::thread::Builder::new()
            .name("mw-save".into())
            .spawn(move || {
                while let Ok((mut id, mut project)) = rx.recv() {
                    // If edits piled up while the last write was in flight,
                    // only the newest state is worth writing.
                    while let Ok(next) = rx.try_recv() {
                        (id, project) = next;
                    }
                    save(&id, &project);
                }
            })
            .ok();
        tx
    });
    let _ = tx.send((id, project));
}

pub fn delete(id: &str) {
    let list: Vec<ProjectMeta> = read_index().into_iter().filter(|m| m.id != id).collect();
    write_index(&list);
    let _ = std::fs::remove_file(project_path(id));
}

pub fn name_of(id: &str) -> String {
    read_index()
        .into_iter()
        .find(|m| m.id == id)
        .map(|m| m.name)
        .unwrap_or_else(|| "MotionWeight".to_string())
}
