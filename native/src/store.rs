//! Editing state: the project, the selection, playback position, and the undo
//! history. Every mutation that should be undoable snapshots first.

use crate::model::*;
use crate::projects;

/// Composition grid lines the preview snaps to, as fractions of the frame.
pub const SNAP_TARGETS: [f32; 5] = [0.0, 1.0 / 3.0, 0.5, 2.0 / 3.0, 1.0];

pub struct Store {
    pub project_id: Option<String>,
    pub project: Project,
    pub selection: Option<Selection>,
    pub playhead: f32,
    pub playing: bool,
    pub playback_rate: f32,
    pub loop_playback: bool,
    pub px_per_sec: f32,
    pub scroll_x: f32,
    /// Composition grid overlay on the preview (always drawn while dragging).
    pub show_grid: bool,
    /// Snap clip edges and centre to the grid while dragging or resizing.
    pub snap_to_grid: bool,
    /// True while the inspector's mask tab is open, which puts mask handles on the preview.
    pub mask_editing: bool,
    /// Set while a clip is being dragged, so lanes don't vanish under the pointer.
    pub suspend_track_cleanup: bool,

    dirty: bool,
    last_save: std::time::Instant,
    undo_stack: Vec<Project>,
    redo_stack: Vec<Project>,
    last_snapshot: std::time::Instant,
    history_group_open: bool,
}

const MAX_HISTORY: usize = 60;

impl Store {
    pub fn new() -> Store {
        Store {
            project_id: None,
            project: Project::default(),
            selection: None,
            playhead: 0.0,
            playing: false,
            playback_rate: 1.0,
            loop_playback: false,
            px_per_sec: 90.0,
            scroll_x: 0.0,
            show_grid: false,
            snap_to_grid: true,
            mask_editing: false,
            suspend_track_cleanup: false,
            dirty: false,
            last_save: std::time::Instant::now(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_snapshot: std::time::Instant::now(),
            history_group_open: false,
        }
    }

    // ---------------------------------------------------------------- history

    /// Groups a continuous interaction (a drag) into a single undo entry.
    pub fn begin_history_group(&mut self) {
        if self.history_group_open {
            return;
        }
        self.snapshot_forced();
        self.history_group_open = true;
    }

    pub fn end_history_group(&mut self) {
        self.history_group_open = false;
        self.last_snapshot = std::time::Instant::now();
    }

    /// Records an undo point. Call before a mutation. Rapid repeats (slider
    /// drags) coalesce into one entry.
    pub fn snapshot(&mut self) {
        if self.history_group_open {
            return;
        }
        if self.last_snapshot.elapsed().as_millis() < 400 {
            return;
        }
        self.push_undo();
    }

    pub fn snapshot_forced(&mut self) {
        if self.history_group_open {
            return;
        }
        self.push_undo();
    }

    fn push_undo(&mut self) {
        self.undo_stack.push(self.project.clone());
        if self.undo_stack.len() > MAX_HISTORY {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
        self.last_snapshot = std::time::Instant::now();
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn undo(&mut self) {
        if let Some(prev) = self.undo_stack.pop() {
            self.redo_stack.push(self.project.clone());
            self.restore(prev);
        }
    }

    pub fn redo(&mut self) {
        if let Some(next) = self.redo_stack.pop() {
            self.undo_stack.push(self.project.clone());
            self.restore(next);
        }
    }

    /// Swaps in a snapshot, keeping the selection when that clip still exists —
    /// otherwise an undo mid-edit would kick you out of the inspector.
    fn restore(&mut self, snapshot: Project) {
        let was = self.selection.map(|s| s.clip_id);
        self.project = snapshot;
        self.selection = None;
        if let Some(clip_id) = was {
            if let Some((track_id, _)) = self.locate(clip_id) {
                self.selection = Some(Selection { track_id, clip_id });
            }
        }
        self.last_snapshot = std::time::Instant::now() - std::time::Duration::from_secs(1);
        self.touch();
    }

    // ------------------------------------------------------------ persistence

    pub fn touch(&mut self) {
        self.dirty = true;
    }

    /// Debounced autosave; call once per frame.
    pub fn maybe_save(&mut self) {
        if !self.dirty || self.last_save.elapsed().as_millis() < 400 {
            return;
        }
        if let Some(id) = &self.project_id {
            projects::save_async(id.clone(), self.project.clone());
        }
        self.dirty = false;
        self.last_save = std::time::Instant::now();
    }

    pub fn save_now(&mut self) {
        if let Some(id) = &self.project_id {
            projects::save(id, &self.project);
        }
        self.dirty = false;
    }

    pub fn load(&mut self, id: &str, project: Project) {
        self.project_id = Some(id.to_string());
        self.project = project;
        self.reseed_ids();
        self.normalize_tracks();
        self.selection = None;
        self.playhead = 0.0;
        self.playing = false;
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.dirty = false;
    }

    /// Loaded ids come from a previous run's counter, so walk the allocator past them.
    fn reseed_ids(&mut self) {
        let mut max = 0;
        for a in &self.project.assets {
            max = max.max(a.id);
        }
        for t in &self.project.tracks {
            max = max.max(t.id);
            for c in &t.clips {
                max = max.max(c.id);
            }
        }
        bump_id_floor(max);
    }

    // ------------------------------------------------------------------ query

    pub fn total_duration(&self) -> f32 {
        self.project
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .map(|c| c.end())
            .fold(0.0_f32, f32::max)
    }

    pub fn asset(&self, id: Id) -> Option<&MediaAsset> {
        self.project.assets.iter().find(|a| a.id == id)
    }

    pub fn track(&self, id: Id) -> Option<&Track> {
        self.project.tracks.iter().find(|t| t.id == id)
    }

    pub fn track_mut(&mut self, id: Id) -> Option<&mut Track> {
        self.project.tracks.iter_mut().find(|t| t.id == id)
    }

    /// `(track id, clip index)` for a clip id.
    pub fn locate(&self, clip_id: Id) -> Option<(Id, usize)> {
        for t in &self.project.tracks {
            if let Some(i) = t.clips.iter().position(|c| c.id == clip_id) {
                return Some((t.id, i));
            }
        }
        None
    }

    pub fn clip(&self, clip_id: Id) -> Option<&Clip> {
        self.project.tracks.iter().find_map(|t| t.clips.iter().find(|c| c.id == clip_id))
    }

    pub fn clip_mut(&mut self, clip_id: Id) -> Option<&mut Clip> {
        self.project.tracks.iter_mut().find_map(|t| t.clips.iter_mut().find(|c| c.id == clip_id))
    }

    pub fn selected_clip(&self) -> Option<&Clip> {
        self.selection.and_then(|s| self.clip(s.clip_id))
    }

    pub fn select(&mut self, sel: Option<Selection>) {
        self.selection = sel;
    }

    /// Clips playing at `time`, back to front. Tracks are stored top-first, so
    /// this walks from the bottom lane upwards: later entries paint over earlier.
    pub fn active_at(&self, time: f32) -> Vec<(Id, Id)> {
        let mut out = Vec::new();
        for track in self.project.tracks.iter().rev() {
            for clip in &track.clips {
                if clip.covers(time) {
                    out.push((track.id, clip.id));
                }
            }
        }
        out
    }

    /// Tracks in composite order (bottom layer first), for the exporter.
    pub fn tracks_bottom_first(&self) -> impl Iterator<Item = &Track> {
        self.project.tracks.iter().rev()
    }

    // ----------------------------------------------------------------- tracks

    /// Creates a track. Tracks are ordered top-first: `tracks[0]` is the topmost
    /// lane and the topmost composite layer. New visual tracks go on top so a
    /// freshly dropped clip is visible; audio settles at the bottom.
    pub fn add_track(&mut self, kind: Option<ClipKind>, index: Option<usize>) -> Id {
        self.snapshot_forced();
        let track = Track { id: next_id(), kind, clips: Vec::new() };
        let id = track.id;
        let at = index.unwrap_or(if kind == Some(ClipKind::Audio) { self.project.tracks.len() } else { 0 });
        let at = at.min(self.project.tracks.len());
        self.project.tracks.insert(at, track);
        self.touch();
        id
    }

    /// A track takes any clip of its own family; an untyped track takes anything.
    pub fn track_accepts(&self, track: &Track, kind: ClipKind) -> bool {
        match track.kind {
            None => true,
            Some(k) => track_family(k) == track_family(kind),
        }
    }

    /// Finds (or creates) a track compatible with `kind`.
    pub fn track_for(&mut self, kind: ClipKind, prefer: Option<Id>) -> Id {
        if let Some(pid) = prefer {
            if let Some(t) = self.track(pid) {
                if self.track_accepts(t, kind) {
                    return pid;
                }
            }
        }
        let existing = self
            .project
            .tracks
            .iter()
            .find(|t| t.kind.is_some() && self.track_accepts(t, kind))
            .map(|t| t.id);
        if let Some(id) = existing {
            return id;
        }
        if let Some(t) = self.project.tracks.iter_mut().find(|t| t.kind.is_none()) {
            t.kind = Some(kind);
            return t.id;
        }
        self.add_track(Some(kind), None)
    }

    pub fn remove_track_if_empty(&mut self, track_id: Id) {
        if self.suspend_track_cleanup || self.project.tracks.len() <= 1 {
            return;
        }
        if self.track(track_id).map(|t| t.clips.is_empty()).unwrap_or(false) {
            self.project.tracks.retain(|t| t.id != track_id);
            self.touch();
        }
    }

    /// Drops every empty lane but the last, once a drag has finished.
    pub fn cleanup_empty_tracks(&mut self) {
        if self.project.tracks.len() <= 1 {
            return;
        }
        let kept: Vec<Track> = self.project.tracks.iter().filter(|t| !t.clips.is_empty()).cloned().collect();
        if kept.len() == self.project.tracks.len() {
            return;
        }
        self.project.tracks = if kept.is_empty() {
            self.project.tracks[..1].to_vec()
        } else {
            kept
        };
        self.touch();
    }

    /// Slides overlapping clips apart, enforcing the no-overlap rule on load.
    fn normalize_tracks(&mut self) {
        for track in &mut self.project.tracks {
            track.clips.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
            let mut cursor = 0.0_f32;
            for clip in &mut track.clips {
                if clip.start < cursor - 1e-6 {
                    clip.start = cursor;
                }
                cursor = clip.end();
            }
        }
    }

    // ------------------------------------------------------------------ clips

    /// Nearest start at which `duration` fits in `track_id` without covering
    /// another clip. Clips never overlap within a track, so a move that would
    /// collide slides into the closest free gap instead.
    pub fn placement_for(&self, track_id: Id, exclude: Option<Id>, desired: f32, duration: f32) -> f32 {
        let want = desired.max(0.0);
        let Some(track) = self.track(track_id) else { return want };

        let mut others: Vec<&Clip> = track.clips.iter().filter(|c| Some(c.id) != exclude).collect();
        others.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));

        // (from, to); the final gap is open-ended.
        let mut gaps: Vec<(f32, f32)> = Vec::new();
        let mut cursor = 0.0_f32;
        for c in &others {
            if c.start - cursor >= duration - 1e-6 {
                gaps.push((cursor, c.start));
            }
            cursor = cursor.max(c.end());
        }
        gaps.push((cursor, f32::INFINITY));

        let mut best = cursor;
        let mut best_dist = f32::INFINITY;
        for (from, to) in gaps {
            let hi = if to.is_infinite() { f32::INFINITY } else { to - duration };
            if hi < from {
                continue;
            }
            let cand = want.max(from).min(hi);
            let dist = (cand - want).abs();
            if dist < best_dist {
                best_dist = dist;
                best = cand;
            }
        }
        best
    }

    /// How far a clip's edges can move before touching its neighbours.
    pub fn neighbor_bounds(&self, clip_id: Id) -> (f32, f32) {
        let Some((track_id, _)) = self.locate(clip_id) else { return (0.0, f32::INFINITY) };
        let Some(track) = self.track(track_id) else { return (0.0, f32::INFINITY) };
        let Some(clip) = track.clips.iter().find(|c| c.id == clip_id) else { return (0.0, f32::INFINITY) };
        let mut prev_end = 0.0_f32;
        let mut next_start = f32::INFINITY;
        for c in &track.clips {
            if c.id == clip_id {
                continue;
            }
            if c.end() <= clip.start + 1e-6 {
                prev_end = prev_end.max(c.end());
            } else if c.start >= clip.end() - 1e-6 {
                next_start = next_start.min(c.start);
            }
        }
        (prev_end, next_start)
    }

    pub fn add_asset(&mut self, asset: MediaAsset) {
        self.project.assets.push(asset);
        self.touch();
    }

    pub fn add_clip(&mut self, track_id: Id, mut clip: Clip) -> Option<Id> {
        self.track(track_id)?;
        self.snapshot_forced();
        clip.start = self.placement_for(track_id, Some(clip.id), clip.start, clip.duration);
        let id = clip.id;
        let kind = clip.kind;
        if let Some(t) = self.track_mut(track_id) {
            if t.kind.is_none() {
                t.kind = Some(kind);
            }
            t.clips.push(clip);
        }
        self.selection = Some(Selection { track_id, clip_id: id });
        self.touch();
        Some(id)
    }

    pub fn move_clip(&mut self, clip_id: Id, to_track: Id, new_start: f32) {
        let Some((from_track, idx)) = self.locate(clip_id) else { return };
        let kind = self.project.tracks.iter().find(|t| t.id == from_track).unwrap().clips[idx].kind;
        let duration = self.project.tracks.iter().find(|t| t.id == from_track).unwrap().clips[idx].duration;
        let Some(target) = self.track(to_track) else { return };
        if !self.track_accepts(target, kind) {
            return;
        }
        let start = self.placement_for(to_track, Some(clip_id), new_start, duration);

        if from_track == to_track {
            if let Some(t) = self.track_mut(to_track) {
                t.clips[idx].start = start;
            }
        } else {
            let mut clip = self.track_mut(from_track).unwrap().clips.remove(idx);
            clip.start = start;
            if let Some(t) = self.track_mut(to_track) {
                if t.kind.is_none() {
                    t.kind = Some(kind);
                }
                t.clips.push(clip);
            }
            self.selection = Some(Selection { track_id: to_track, clip_id });
            self.remove_track_if_empty(from_track);
        }
        self.touch();
    }

    pub fn remove_clip(&mut self, clip_id: Id) {
        let Some((track_id, idx)) = self.locate(clip_id) else { return };
        self.snapshot_forced();
        self.track_mut(track_id).unwrap().clips.remove(idx);
        if self.selection.map(|s| s.clip_id) == Some(clip_id) {
            self.selection = None;
        }
        self.remove_track_if_empty(track_id);
        self.touch();
    }

    /// Splits the clip at absolute timeline position `at` into two clips.
    pub fn split_clip(&mut self, clip_id: Id, at: f32) -> bool {
        let Some((track_id, idx)) = self.locate(clip_id) else { return false };
        let clip = &self.project.tracks.iter().find(|t| t.id == track_id).unwrap().clips[idx];
        let offset = at - clip.start;
        if offset <= 0.05 || offset >= clip.duration - 0.05 {
            return false;
        }
        self.snapshot_forced();
        let track = self.track_mut(track_id).unwrap();
        let mut right = track.clips[idx].clone();
        right.id = next_id();
        right.start = at;
        right.duration = track.clips[idx].duration - offset;
        if right.kind != ClipKind::Text {
            right.trim_in += offset;
        }
        track.clips[idx].duration = offset;
        let right_id = right.id;
        track.clips.insert(idx + 1, right);
        self.selection = Some(Selection { track_id, clip_id: right_id });
        self.touch();
        true
    }

    /// Deletes the portion of a clip on one side of the playhead. `Left` keeps
    /// everything after `at`; `Right` keeps everything before it.
    pub fn trim_to(&mut self, clip_id: Id, at: f32, keep_right: bool) -> bool {
        let Some(clip) = self.clip(clip_id) else { return false };
        let offset = at - clip.start;
        if offset <= 0.05 || offset >= clip.duration - 0.05 {
            return false;
        }
        self.snapshot_forced();
        let clip = self.clip_mut(clip_id).unwrap();
        if keep_right {
            if clip.kind != ClipKind::Text {
                clip.trim_in += offset;
            }
            clip.start = at;
            clip.duration -= offset;
        } else {
            clip.duration = offset;
        }
        self.touch();
        true
    }

    pub fn set_playhead(&mut self, t: f32) {
        let max = self.total_duration().max(0.001);
        self.playhead = t.clamp(0.0, max);
    }
}
