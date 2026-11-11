//! Floating sprite window: expand/collapse for the radial menu and position persistence.
//!
//! Geometry lives in `geometry.rs` (pure, unit-tested). This module only talks to Tauri.
//!
//! Threading rules (important on Windows):
//!  - Window getters (`outer_position`, `current_monitor`, ...) are synchronous round trips to
//!    the event-loop thread. They must never be called while the event-loop thread could be
//!    blocked on a lock we hold. The `Moved` handler therefore runs on the event-loop thread and
//!    only touches atomics and a channel, never `FloatingState::mode`.
//!  - `set_floating_expanded` is `async` so it runs on the async runtime, not the main thread.

use std::{
    fs,
    io::Write as _,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
        Mutex, MutexGuard,
    },
    time::Duration,
};

use serde::Serialize;
use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, Position, Size, WebviewWindow};

use crate::{
    geometry::{
        self, MonitorGeom, PhysRect, Placement, ResizePlan, SavedPosition, COLLAPSED_LOGICAL,
        EXPANDED_LOGICAL,
    },
    CommandError, LABEL_FLOATING,
};

/// Quiet period after the last `Moved` event before the position is written.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(600);
const POSITION_FILE: &str = "floating-position.json";

/// What the page gets back from `set_floating_expanded` (matches the agreed payload).
#[derive(Clone, Copy, Debug, Serialize, PartialEq)]
pub struct ExpandResult {
    /// Logical size of the window after the call.
    pub width: f64,
    pub height: f64,
    /// Logical offset of the sprite centre relative to the window centre (0,0 unless clamped).
    pub sprite_offset_x: f64,
    pub sprite_offset_y: f64,
}

impl ExpandResult {
    fn from_plan(p: &ResizePlan) -> Self {
        Self {
            width: p.logical_size.0,
            height: p.logical_size.1,
            sprite_offset_x: p.sprite_offset.0,
            sprite_offset_y: p.sprite_offset.1,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Expanded {
    /// Window position/size as we last set them. If the live window still matches, the
    /// window has not been dragged since and the saved collapsed rect can be restored exactly.
    applied: PhysRect,
    /// The collapsed rect we came from (None after the user dragged the expanded window).
    collapsed: Option<PhysRect>,
    offset: (f64, f64),
    result: ExpandResult,
}

#[derive(Clone, Copy, Debug)]
enum Mode {
    Collapsed,
    Expanded(Expanded),
}

pub struct FloatingState {
    mode: Mutex<Mode>,
    /// Mirror of `mode` for the event-loop thread (see module docs).
    expanded_flag: AtomicBool,
    moved_tx: Sender<()>,
    moved_rx: Mutex<Option<Receiver<()>>>,
    last_saved: Mutex<Option<SavedPosition>>,
}

impl Default for FloatingState {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            mode: Mutex::new(Mode::Collapsed),
            expanded_flag: AtomicBool::new(false),
            moved_tx: tx,
            moved_rx: Mutex::new(Some(rx)),
            last_saved: Mutex::new(None),
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn external<E: std::fmt::Display>(what: &str, e: E) -> CommandError {
    CommandError::new("external_failure", format!("{what}: {e}"))
}

fn to_geom(m: &tauri::window::Monitor) -> MonitorGeom {
    let w = m.work_area();
    MonitorGeom {
        name: m.name().cloned(),
        work: PhysRect::new(w.position.x, w.position.y, w.size.width, w.size.height),
        scale: m.scale_factor(),
    }
}

/// Monitor holding the point (physical px), else the window's current monitor, else primary.
fn monitor_for_point(win: &WebviewWindow, px: f64, py: f64) -> Option<tauri::window::Monitor> {
    win.monitor_from_point(px, py)
        .ok()
        .flatten()
        .or_else(|| win.current_monitor().ok().flatten())
        .or_else(|| win.primary_monitor().ok().flatten())
}

fn live_rect(win: &WebviewWindow) -> Result<PhysRect, CommandError> {
    let p = win
        .outer_position()
        .map_err(|e| external("outer_position", e))?;
    let s = win.outer_size().map_err(|e| external("outer_size", e))?;
    Ok(PhysRect::new(p.x, p.y, s.width, s.height))
}

fn apply(win: &WebviewWindow, plan: &ResizePlan, expanding: bool) -> Result<(), CommandError> {
    let size = Size::Physical(PhysicalSize::new(plan.size.0, plan.size.1));
    let pos = Position::Physical(PhysicalPosition::new(plan.pos.0, plan.pos.1));
    // Tauri has no combined "set bounds" for top-level windows, so this is two SetWindowPos
    // calls (both SWP_NOACTIVATE | SWP_NOZORDER | SWP_ASYNCWINDOWPOS in tao 0.37.2) queued
    // back to back on the event loop. Neither takes focus nor touches always-on-top.
    if expanding {
        win.set_size(size).map_err(|e| external("set_size", e))?;
        win.set_position(pos)
            .map_err(|e| external("set_position", e))?;
    } else {
        win.set_position(pos)
            .map_err(|e| external("set_position", e))?;
        win.set_size(size).map_err(|e| external("set_size", e))?;
    }
    Ok(())
}

/// Shell side of "hover the sprite -> radial menu".
///
/// Only the floating window may call it (checked here and by the capability file). Calls are
/// idempotent and order-independent: the sprite centre is always derived from the live window
/// rectangle plus the stored offset, never from "what the previous call did".
#[tauri::command]
pub async fn set_floating_expanded(
    caller: WebviewWindow,
    app: AppHandle,
    expanded: bool,
) -> Result<ExpandResult, CommandError> {
    if caller.label() != LABEL_FLOATING {
        return Err(CommandError::new(
            "permission_denied",
            "only the floating window may resize itself",
        ));
    }
    let state = app.state::<FloatingState>();
    // Serialises concurrent calls; held across the (blocking) getters on purpose.
    let mut mode = lock(&state.mode);
    let result = set_expanded_locked(&caller, &mut mode, expanded)?;
    state.expanded_flag.store(expanded, Ordering::SeqCst);
    Ok(result)
}

fn set_expanded_locked(
    win: &WebviewWindow,
    mode: &mut Mode,
    expanded: bool,
) -> Result<ExpandResult, CommandError> {
    let cur = live_rect(win)?;
    let offset_now = match mode {
        Mode::Collapsed => (0.0, 0.0),
        Mode::Expanded(e) => e.offset,
    };

    match (expanded, *mode) {
        // Already expanded and not moved since: nothing to do.
        (true, Mode::Expanded(e)) if e.applied == cur => return Ok(e.result),
        // Collapsed and already collapsed-sized: nothing to do (even after a drag).
        (false, Mode::Collapsed) => {
            let s = win
                .scale_factor()
                .map_err(|e| external("scale_factor", e))?;
            let want = (COLLAPSED_LOGICAL * s).round() as u32;
            if cur.w.abs_diff(want) <= 1 && cur.h.abs_diff(want) <= 1 {
                return Ok(ExpandResult {
                    width: COLLAPSED_LOGICAL,
                    height: COLLAPSED_LOGICAL,
                    sprite_offset_x: 0.0,
                    sprite_offset_y: 0.0,
                });
            }
        }
        _ => {}
    }

    // Exact restore when collapsing an untouched expanded window.
    if !expanded {
        if let Mode::Expanded(e) = *mode {
            if let (true, Some(c)) = (e.applied == cur, e.collapsed) {
                let plan = ResizePlan {
                    pos: (c.x, c.y),
                    size: (c.w, c.h),
                    logical_size: (COLLAPSED_LOGICAL, COLLAPSED_LOGICAL),
                    sprite_offset: (0.0, 0.0),
                };
                apply(win, &plan, false)?;
                *mode = Mode::Collapsed;
                return Ok(ExpandResult::from_plan(&plan));
            }
        }
    }

    // General path: sprite centre = live centre + stored offset; pick the monitor under it.
    let (cx, cy) = cur.center();
    let fallback_scale = win
        .scale_factor()
        .map_err(|e| external("scale_factor", e))?;
    let probe = (
        cx + offset_now.0 * fallback_scale,
        cy + offset_now.1 * fallback_scale,
    );
    let monitor = monitor_for_point(win, probe.0, probe.1).map(|m| to_geom(&m));
    let scale = monitor.as_ref().map(|m| m.scale).unwrap_or(fallback_scale);
    let work = monitor.as_ref().map(|m| m.work);

    let target = if expanded {
        (EXPANDED_LOGICAL, EXPANDED_LOGICAL)
    } else {
        (COLLAPSED_LOGICAL, COLLAPSED_LOGICAL)
    };
    // Clamp only when expanding; collapsing must leave the sprite exactly where it is.
    let plan = geometry::plan_resize(cur, offset_now, target, scale, work, expanded);
    apply(win, &plan, expanded)?;

    let result = ExpandResult::from_plan(&plan);
    *mode = if expanded {
        let collapsed = match *mode {
            Mode::Collapsed => Some(cur),
            Mode::Expanded(_) => None, // dragged while expanded: no exact restore any more
        };
        Mode::Expanded(Expanded {
            applied: PhysRect::new(plan.pos.0, plan.pos.1, plan.size.0, plan.size.1),
            collapsed,
            offset: plan.sprite_offset,
            result,
        })
    } else {
        Mode::Collapsed
    };
    Ok(result)
}

// ---------------------------------------------------------------------------------------
// Position persistence
// ---------------------------------------------------------------------------------------

fn position_path(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_config_dir()
        .ok()
        .map(|d| d.join(POSITION_FILE))
}

/// Writes `bytes` to `path` via a temp file in the same directory, then renames over it, so a
/// crash mid-write leaves either the old or the new file, never half of one.
fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    fs::create_dir_all(dir)?;
    let tmp = path.with_extension("json.tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    // std::fs::rename replaces an existing destination on Windows (MoveFileExW with
    // MOVEFILE_REPLACE_EXISTING).
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

fn read_saved(path: &std::path::Path) -> Option<SavedPosition> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Called once from `setup`: puts the floating window at its saved spot (or the default spot
/// when nothing is saved or the saved spot is off every current monitor) and starts the
/// debounced save thread. The window is still hidden here; `on_page_load` reveals it.
pub fn init(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(LABEL_FLOATING) {
        restore_position(app, &win);
    }
    let state = app.state::<FloatingState>();
    let rx = lock(&state.moved_rx).take();
    if let Some(rx) = rx {
        let app = app.clone();
        std::thread::Builder::new()
            .name("luma-floating-save".into())
            .spawn(move || save_loop(app, rx))
            .ok();
    }
}

fn restore_position(app: &AppHandle, win: &WebviewWindow) {
    let monitors = match win.available_monitors() {
        Ok(m) => m,
        Err(e) => {
            eprintln!("[lumanote] available_monitors failed: {e}");
            return;
        }
    };
    let geoms: Vec<MonitorGeom> = monitors.iter().map(to_geom).collect();
    // Index of the primary monitor, matched by name + work area.
    let primary = win.primary_monitor().ok().flatten().and_then(|p| {
        let pg = to_geom(&p);
        geoms.iter().position(|g| *g == pg)
    });
    let saved = position_path(app).and_then(|p| read_saved(&p));
    if let Some(s) = &saved {
        *lock(&app.state::<FloatingState>().last_saved) = Some(s.clone());
    }
    let Some(((x, y), how)) = geometry::resolve_position(saved.as_ref(), &geoms, primary) else {
        return;
    };
    // Scale of the monitor that will hold the window, to size it exactly.
    let target = geoms.iter().find(|g| {
        g.work
            .contains_point(f64::from(x) + 1.0, f64::from(y) + 1.0)
    });
    if let Err(e) = win.set_position(Position::Physical(PhysicalPosition::new(x, y))) {
        eprintln!("[lumanote] restore set_position failed: {e}");
    }
    if let Some(g) = target {
        let side = (COLLAPSED_LOGICAL * g.scale).round() as u32;
        let _ = win.set_size(Size::Physical(PhysicalSize::new(side, side)));
    }
    if how == Placement::Default {
        eprintln!("[lumanote] floating window: using default position ({x}, {y})");
    }
}

/// Called from the `Moved` window event, on the event-loop thread. Must stay non-blocking.
pub fn note_moved(app: &AppHandle) {
    let state = app.state::<FloatingState>();
    if state.expanded_flag.load(Ordering::SeqCst) {
        return;
    }
    let _ = state.moved_tx.send(());
}

fn save_loop(app: AppHandle, rx: Receiver<()>) {
    while rx.recv().is_ok() {
        loop {
            match rx.recv_timeout(SAVE_DEBOUNCE) {
                Ok(()) => continue,
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        save_now(&app);
    }
}

fn save_now(app: &AppHandle) {
    let Some(win) = app.get_webview_window(LABEL_FLOATING) else {
        return;
    };
    let state = app.state::<FloatingState>();
    // Holding `mode` keeps a concurrent expand/collapse from interleaving with the reads below.
    let mode = lock(&state.mode);
    if matches!(*mode, Mode::Expanded(_)) {
        return; // never persist the expanded window's position
    }
    let Ok(cur) = live_rect(&win) else { return };
    let Ok(scale) = win.scale_factor() else {
        return;
    };
    let want = (COLLAPSED_LOGICAL * scale).round() as u32;
    if cur.w.abs_diff(want) > 2 || cur.h.abs_diff(want) > 2 {
        return; // mid-transition or not collapsed-sized
    }
    let (cx, cy) = cur.center();
    let Some(monitor) = monitor_for_point(&win, cx, cy).map(|m| to_geom(&m)) else {
        return;
    };
    let record = geometry::to_saved((cur.x, cur.y), &monitor);
    if lock(&state.last_saved).as_ref() == Some(&record) {
        return;
    }
    let Some(path) = position_path(app) else {
        return;
    };
    let Ok(json) = serde_json::to_vec_pretty(&record) else {
        return;
    };
    match write_atomic(&path, &json) {
        Ok(()) => *lock(&state.last_saved) = Some(record),
        Err(e) => eprintln!("[lumanote] saving floating position failed: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_replaces_and_leaves_no_tmp() {
        let dir = std::env::temp_dir().join(format!("lumanote-shell-test-{}", std::process::id()));
        let path = dir.join("nested").join(POSITION_FILE);
        write_atomic(&path, b"{\"a\":1}").unwrap();
        write_atomic(&path, b"{\"a\":2}").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "{\"a\":2}");
        assert!(!path.with_extension("json.tmp").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_saved_tolerates_garbage_and_missing() {
        let dir = std::env::temp_dir().join(format!("lumanote-shell-test2-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("x.json");
        assert!(read_saved(&p).is_none());
        fs::write(&p, "not json").unwrap();
        assert!(read_saved(&p).is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
