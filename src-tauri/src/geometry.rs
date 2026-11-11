//! Pure window geometry for the floating sprite.
//!
//! Nothing here touches Tauri, so it is unit-tested on Linux (`cargo test -p elf-shell`).
//!
//! Conventions:
//!  - "physical" = virtual-desktop pixels as reported by Tauri (`PhysicalPosition`, work areas).
//!    On Windows the origin is the primary monitor's top-left and monitors left of / above it
//!    have negative coordinates.
//!  - "logical" = physical divided by the scale factor of the monitor the sprite is on.
//!  - Window sizes are specified in logical units (128 / 344) and converted with the scale factor
//!    of that monitor, rounded to whole pixels.

use serde::{Deserialize, Serialize};

// The frontend owns these numbers: src/features/floating/layout.ts (FLOATING_COLLAPSED_PX,
// FLOATING_EXPANDED_PX, and the wheel geometry that decides them). `tauri.conf.json` repeats the
// collapsed size for the initial window. `tests::constants_match_frontend` fails if any copy drifts.
/// Collapsed floating window edge, logical px: the 96 px sprite plus 16 px of room on every side.
pub const COLLAPSED_LOGICAL: f64 = 128.0;
/// Expanded (radial menu) floating window edge, logical px.
pub const EXPANDED_LOGICAL: f64 = 344.0;
/// Collapsed edge before the 2.5D sprite (positions saved by versions up to 1 refer to this window).
const LEGACY_V1_COLLAPSED_LOGICAL: f64 = 96.0;
/// Distance of the default spot from the work-area edges, logical px.
pub const DEFAULT_MARGIN_LOGICAL: f64 = 24.0;

/// A rectangle in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PhysRect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

impl PhysRect {
    pub fn new(x: i32, y: i32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    fn right(&self) -> i64 {
        i64::from(self.x) + i64::from(self.w)
    }

    fn bottom(&self) -> i64 {
        i64::from(self.y) + i64::from(self.h)
    }

    /// Centre in physical pixels (may be fractional).
    pub fn center(&self) -> (f64, f64) {
        (
            f64::from(self.x) + f64::from(self.w) / 2.0,
            f64::from(self.y) + f64::from(self.h) / 2.0,
        )
    }

    /// Half-open containment: `x <= px < x + w`.
    pub fn contains_point(&self, px: f64, py: f64) -> bool {
        px >= f64::from(self.x)
            && px < self.right() as f64
            && py >= f64::from(self.y)
            && py < self.bottom() as f64
    }
}

/// What the shell needs to know about one monitor.
#[derive(Clone, Debug, PartialEq)]
pub struct MonitorGeom {
    pub name: Option<String>,
    /// Work area (monitor minus taskbar), physical.
    pub work: PhysRect,
    pub scale: f64,
}

/// Result of a resize computation, all in physical px except where noted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResizePlan {
    pub pos: (i32, i32),
    pub size: (u32, u32),
    /// Logical size that was asked for (what the page lays itself out in).
    pub logical_size: (f64, f64),
    /// Logical offset of the sprite centre relative to the new window centre.
    pub sprite_offset: (f64, f64),
}

fn round_px(v: f64) -> i32 {
    v.round() as i32
}

fn size_px(logical: f64, scale: f64) -> u32 {
    (logical * scale).round().max(1.0) as u32
}

/// Moves `rect` the shortest way so it lies inside `work`. If it is larger than `work` on an
/// axis it is pinned to the work area's start on that axis.
pub fn clamp_into(rect: PhysRect, work: PhysRect) -> (i32, i32) {
    fn axis(pos: i32, len: u32, w_pos: i32, w_len: u32) -> i32 {
        if len >= w_len {
            return w_pos;
        }
        let max = i64::from(w_pos) + i64::from(w_len) - i64::from(len);
        i64::from(pos).clamp(i64::from(w_pos), max) as i32
    }
    (
        axis(rect.x, rect.w, work.x, work.w),
        axis(rect.y, rect.h, work.y, work.h),
    )
}

/// Resizes a window to `target_logical` keeping the *sprite centre* fixed on screen.
///
/// The sprite centre is `current` centre plus `current_offset` (logical; zero while collapsed).
/// With `work` and `clamp` the new window is shifted back into the work area when needed; the
/// shift is returned as `sprite_offset` (sprite centre minus new window centre, logical) so the
/// page can draw the sprite where it was. Collapsing must not clamp: the sprite stays exactly
/// where it is, even if the user parked it half outside the screen.
pub fn plan_resize(
    current: PhysRect,
    current_offset: (f64, f64),
    target_logical: (f64, f64),
    scale: f64,
    work: Option<PhysRect>,
    clamp: bool,
) -> ResizePlan {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let (cx, cy) = current.center();
    // Sprite centre in physical px.
    let sx = cx + current_offset.0 * scale;
    let sy = cy + current_offset.1 * scale;

    let w = size_px(target_logical.0, scale);
    let h = size_px(target_logical.1, scale);
    let mut x = round_px(sx - f64::from(w) / 2.0);
    let mut y = round_px(sy - f64::from(h) / 2.0);

    if clamp {
        if let Some(work) = work {
            (x, y) = clamp_into(PhysRect::new(x, y, w, h), work);
        }
    }

    let new_rect = PhysRect::new(x, y, w, h);
    let (ncx, ncy) = new_rect.center();
    ResizePlan {
        pos: (x, y),
        size: (w, h),
        logical_size: target_logical,
        sprite_offset: ((sx - ncx) / scale, (sy - ncy) / scale),
    }
}

/// Position of the collapsed window stored on disk. Logical coordinates plus the monitor
/// identity and scale (the window-state plugin stores physical coordinates, which break when
/// the scale or monitor layout changes; R1 section 3.3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedPosition {
    pub version: u32,
    /// Windows device name of the monitor at save time (`\\.\DISPLAY1`), if known.
    pub monitor_name: Option<String>,
    /// Scale factor of that monitor at save time.
    pub scale_factor: f64,
    /// Absolute top-left, logical = physical / `scale_factor`.
    pub x: f64,
    pub y: f64,
    /// Top-left relative to that monitor's work-area origin, logical.
    pub rel_x: f64,
    pub rel_y: f64,
}

/// Version 2 = top-left of the 128 px window. Version 1 files (96 px window) are still read and migrated.
pub const SAVED_POSITION_VERSION: u32 = 2;

/// Brings a stored record to the current window size, keeping the SPRITE CENTRE where it was.
///
/// A version-1 record holds the top-left of a 96 px window; the window is now larger and is resized
/// around the same centre, so the top-left moves up/left by half the difference (16 logical px).
/// Other versions are refused (`None`).
pub fn migrate_saved(s: &SavedPosition) -> Option<SavedPosition> {
    match s.version {
        SAVED_POSITION_VERSION => Some(s.clone()),
        1 => {
            let shift = (COLLAPSED_LOGICAL - LEGACY_V1_COLLAPSED_LOGICAL) / 2.0;
            Some(SavedPosition {
                version: SAVED_POSITION_VERSION,
                x: s.x - shift,
                y: s.y - shift,
                rel_x: s.rel_x - shift,
                rel_y: s.rel_y - shift,
                ..s.clone()
            })
        }
        _ => None,
    }
}

/// Builds the record for a collapsed window at `pos` on `monitor`.
pub fn to_saved(pos: (i32, i32), monitor: &MonitorGeom) -> SavedPosition {
    let s = if monitor.scale > 0.0 {
        monitor.scale
    } else {
        1.0
    };
    SavedPosition {
        version: SAVED_POSITION_VERSION,
        monitor_name: monitor.name.clone(),
        scale_factor: s,
        x: f64::from(pos.0) / s,
        y: f64::from(pos.1) / s,
        rel_x: f64::from(pos.0 - monitor.work.x) / s,
        rel_y: f64::from(pos.1 - monitor.work.y) / s,
    }
}

/// Where a restored position came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// Same-named monitor still present; position kept relative to it.
    SameMonitor,
    /// Monitor name unknown but the old absolute spot is on some current monitor.
    Absolute,
    /// Nothing usable: default spot near the bottom-right of the primary work area.
    Default,
}

/// Default spot: bottom-right of `work`, `DEFAULT_MARGIN_LOGICAL` from the edges.
pub fn default_position(work: PhysRect, scale: f64) -> (i32, i32) {
    let w = size_px(COLLAPSED_LOGICAL, scale);
    let m = round_px(DEFAULT_MARGIN_LOGICAL * scale);
    let rect = PhysRect::new(
        round_px(work.right() as f64) - w as i32 - m,
        round_px(work.bottom() as f64) - w as i32 - m,
        w,
        w,
    );
    clamp_into(rect, work)
}

/// Decides where the collapsed window goes at start-up.
///
/// Returns `None` only when there are no monitors at all (leave the window where the OS put it).
/// `primary` indexes `monitors`.
pub fn resolve_position(
    saved: Option<&SavedPosition>,
    monitors: &[MonitorGeom],
    primary: Option<usize>,
) -> Option<((i32, i32), Placement)> {
    if monitors.is_empty() {
        return None;
    }
    let migrated = saved.and_then(migrate_saved);
    if let Some(s) = migrated.as_ref().filter(|s| is_sane(s)) {
        // 1. Same monitor by name: keep the spot relative to it, scaled with its current scale,
        //    and pull it fully into its work area (resolution / taskbar may have changed).
        if let Some(name) = &s.monitor_name {
            if let Some(m) = monitors.iter().find(|m| m.name.as_ref() == Some(name)) {
                let w = size_px(COLLAPSED_LOGICAL, m.scale);
                let x = m.work.x + round_px(s.rel_x * m.scale);
                let y = m.work.y + round_px(s.rel_y * m.scale);
                let pos = clamp_into(PhysRect::new(x, y, w, w), m.work);
                return Some((pos, Placement::SameMonitor));
            }
        }
        // 2. Old absolute spot, if its centre is on a monitor that exists now.
        let w_old = size_px(COLLAPSED_LOGICAL, s.scale_factor);
        let x = round_px(s.x * s.scale_factor);
        let y = round_px(s.y * s.scale_factor);
        let old = PhysRect::new(x, y, w_old, w_old);
        let (cx, cy) = old.center();
        if let Some(m) = monitors.iter().find(|m| m.work.contains_point(cx, cy)) {
            let w = size_px(COLLAPSED_LOGICAL, m.scale);
            let pos = clamp_into(PhysRect::new(x, y, w, w), m.work);
            return Some((pos, Placement::Absolute));
        }
    }
    // 3. Default near the bottom-right of the primary work area.
    let m = primary
        .and_then(|i| monitors.get(i))
        .unwrap_or(&monitors[0]);
    Some((default_position(m.work, m.scale), Placement::Default))
}

fn is_sane(s: &SavedPosition) -> bool {
    [s.scale_factor, s.x, s.y, s.rel_x, s.rel_y]
        .iter()
        .all(|v| v.is_finite())
        && s.scale_factor > 0.0
        && s.scale_factor < 16.0
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCALES: [f64; 4] = [1.0, 1.25, 1.5, 2.0];

    fn work_1080(scale: f64) -> PhysRect {
        // 1920x1080 physical monitor with a 48 px taskbar at the bottom.
        let _ = scale;
        PhysRect::new(0, 0, 1920, 1032)
    }

    fn collapsed(x: i32, y: i32, scale: f64) -> PhysRect {
        let s = size_px(COLLAPSED_LOGICAL, scale);
        PhysRect::new(x, y, s, s)
    }

    fn sprite_center(rect: PhysRect, offset: (f64, f64), scale: f64) -> (f64, f64) {
        let (cx, cy) = rect.center();
        (cx + offset.0 * scale, cy + offset.1 * scale)
    }

    fn approx(a: (f64, f64), b: (f64, f64)) {
        assert!(
            (a.0 - b.0).abs() < 0.51 && (a.1 - b.1).abs() < 0.51,
            "{a:?} != {b:?}"
        );
    }

    #[test]
    fn sizes_follow_scale_factor() {
        for (s, c, e) in [
            (1.0, 128, 344),
            (1.25, 160, 430),
            (1.5, 192, 516),
            (2.0, 256, 688),
        ] {
            assert_eq!(size_px(COLLAPSED_LOGICAL, s), c);
            assert_eq!(size_px(EXPANDED_LOGICAL, s), e);
        }
    }

    #[test]
    fn expand_in_open_space_keeps_sprite_fixed() {
        for s in SCALES {
            let cur = collapsed(800, 400, s);
            let p = plan_resize(
                cur,
                (0.0, 0.0),
                (EXPANDED_LOGICAL, EXPANDED_LOGICAL),
                s,
                Some(work_1080(s)),
                true,
            );
            assert_eq!(
                p.size,
                (size_px(EXPANDED_LOGICAL, s), size_px(EXPANDED_LOGICAL, s))
            );
            assert_eq!(p.sprite_offset, (0.0, 0.0), "scale {s}");
            approx(
                PhysRect::new(p.pos.0, p.pos.1, p.size.0, p.size.1).center(),
                cur.center(),
            );
            assert_eq!(p.logical_size, (EXPANDED_LOGICAL, EXPANDED_LOGICAL));
        }
    }

    #[test]
    fn expand_then_collapse_round_trips_for_all_scales() {
        for s in SCALES {
            let cur = collapsed(1000, 600, s);
            let e = plan_resize(
                cur,
                (0.0, 0.0),
                (EXPANDED_LOGICAL, EXPANDED_LOGICAL),
                s,
                Some(work_1080(s)),
                true,
            );
            let exp_rect = PhysRect::new(e.pos.0, e.pos.1, e.size.0, e.size.1);
            let c = plan_resize(
                exp_rect,
                e.sprite_offset,
                (COLLAPSED_LOGICAL, COLLAPSED_LOGICAL),
                s,
                None,
                false,
            );
            assert_eq!(c.pos, (cur.x, cur.y), "scale {s}");
            assert_eq!(c.size, (cur.w, cur.h));
            assert_eq!(c.sprite_offset, (0.0, 0.0));
        }
    }

    #[test]
    fn expand_clamps_at_right_and_bottom_and_reports_offset() {
        for s in SCALES {
            let work = work_1080(s);
            // Collapsed window flush with the bottom-right corner of the work area.
            let c = collapsed(
                work.x + work.w as i32 - size_px(COLLAPSED_LOGICAL, s) as i32,
                work.y + work.h as i32 - size_px(COLLAPSED_LOGICAL, s) as i32,
                s,
            );
            let p = plan_resize(
                c,
                (0.0, 0.0),
                (EXPANDED_LOGICAL, EXPANDED_LOGICAL),
                s,
                Some(work),
                true,
            );
            assert_eq!(
                p.pos.0 + p.size.0 as i32,
                work.x + work.w as i32,
                "scale {s}"
            );
            assert_eq!(p.pos.1 + p.size.1 as i32, work.y + work.h as i32);
            // Sprite is half the size difference right/below the expanded window centre.
            let half = (EXPANDED_LOGICAL - COLLAPSED_LOGICAL) / 2.0;
            assert!(
                (p.sprite_offset.0 - half).abs() < 0.01,
                "{:?}",
                p.sprite_offset
            );
            assert!((p.sprite_offset.1 - half).abs() < 0.01);
            // Sprite did not move on screen.
            approx(
                sprite_center(
                    PhysRect::new(p.pos.0, p.pos.1, p.size.0, p.size.1),
                    p.sprite_offset,
                    s,
                ),
                c.center(),
            );
            // Collapse from the clamped state: exactly back where it was.
            let exp = PhysRect::new(p.pos.0, p.pos.1, p.size.0, p.size.1);
            let back = plan_resize(
                exp,
                p.sprite_offset,
                (COLLAPSED_LOGICAL, COLLAPSED_LOGICAL),
                s,
                None,
                false,
            );
            assert_eq!(back.pos, (c.x, c.y), "scale {s}");
        }
    }

    #[test]
    fn expand_clamps_at_left_top() {
        let s = 1.5;
        let work = PhysRect::new(0, 0, 2560, 1400);
        let c = collapsed(0, 0, s);
        let p = plan_resize(
            c,
            (0.0, 0.0),
            (EXPANDED_LOGICAL, EXPANDED_LOGICAL),
            s,
            Some(work),
            true,
        );
        assert_eq!(p.pos, (0, 0));
        let half = (EXPANDED_LOGICAL - COLLAPSED_LOGICAL) / 2.0;
        assert!((p.sprite_offset.0 + half).abs() < 0.01);
        assert!((p.sprite_offset.1 + half).abs() < 0.01);
    }

    #[test]
    fn negative_origin_monitor_left_of_primary() {
        for s in SCALES {
            // Secondary monitor to the left: x from -1920, taskbar-free work area.
            let work = PhysRect::new(-1920, -200, 1920, 1080);
            let near_left = collapsed(-1920 + 4, 300, s);
            let p = plan_resize(
                near_left,
                (0.0, 0.0),
                (EXPANDED_LOGICAL, EXPANDED_LOGICAL),
                s,
                Some(work),
                true,
            );
            assert_eq!(p.pos.0, -1920, "scale {s}");
            assert!(p.sprite_offset.0 < 0.0);
            // Near the top edge, which is also negative.
            let near_top = collapsed(-1000, -200, s);
            let q = plan_resize(
                near_top,
                (0.0, 0.0),
                (EXPANDED_LOGICAL, EXPANDED_LOGICAL),
                s,
                Some(work),
                true,
            );
            assert_eq!(q.pos.1, -200);
            assert!(q.sprite_offset.1 < 0.0);
            // Sprite fixed.
            approx(
                sprite_center(
                    PhysRect::new(q.pos.0, q.pos.1, q.size.0, q.size.1),
                    q.sprite_offset,
                    s,
                ),
                near_top.center(),
            );
        }
    }

    #[test]
    fn expand_is_idempotent_from_the_expanded_state() {
        let s = 1.25;
        let work = work_1080(s);
        let c = collapsed(work.w as i32 - 130, 500, s);
        let a = plan_resize(
            c,
            (0.0, 0.0),
            (EXPANDED_LOGICAL, EXPANDED_LOGICAL),
            s,
            Some(work),
            true,
        );
        let exp = PhysRect::new(a.pos.0, a.pos.1, a.size.0, a.size.1);
        // Calling expand again from the expanded state yields the same plan.
        let b = plan_resize(
            exp,
            a.sprite_offset,
            (EXPANDED_LOGICAL, EXPANDED_LOGICAL),
            s,
            Some(work),
            true,
        );
        assert_eq!(a.pos, b.pos);
        assert_eq!(a.size, b.size);
        assert!((a.sprite_offset.0 - b.sprite_offset.0).abs() < 1e-9);
        assert!((a.sprite_offset.1 - b.sprite_offset.1).abs() < 1e-9);
    }

    #[test]
    fn collapse_does_not_clamp_a_sprite_parked_off_screen() {
        let s = 1.0;
        let work = PhysRect::new(0, 0, 1920, 1032);
        // Sprite centre at x = 1900 -> window sticks out to the right.
        let c = collapsed(1900 - COLLAPSED_LOGICAL as i32 / 2, 100, s);
        let e = plan_resize(
            c,
            (0.0, 0.0),
            (EXPANDED_LOGICAL, EXPANDED_LOGICAL),
            s,
            Some(work),
            true,
        );
        assert!(e.pos.0 + EXPANDED_LOGICAL as i32 <= 1920);
        let exp = PhysRect::new(e.pos.0, e.pos.1, e.size.0, e.size.1);
        let back = plan_resize(
            exp,
            e.sprite_offset,
            (COLLAPSED_LOGICAL, COLLAPSED_LOGICAL),
            s,
            Some(work),
            false,
        );
        assert_eq!(back.pos, (c.x, c.y));
    }

    #[test]
    fn work_area_smaller_than_expanded_window_pins_to_origin() {
        let work = PhysRect::new(100, 50, 200, 200);
        let c = collapsed(150, 100, 1.0);
        let p = plan_resize(
            c,
            (0.0, 0.0),
            (EXPANDED_LOGICAL, EXPANDED_LOGICAL),
            1.0,
            Some(work),
            true,
        );
        assert_eq!(p.pos, (100, 50));
    }

    #[test]
    fn bad_scale_falls_back_to_one() {
        let c = collapsed(10, 10, 1.0);
        let p = plan_resize(
            c,
            (0.0, 0.0),
            (EXPANDED_LOGICAL, EXPANDED_LOGICAL),
            f64::NAN,
            None,
            true,
        );
        assert_eq!(p.size, (EXPANDED_LOGICAL as u32, EXPANDED_LOGICAL as u32));
    }

    fn mon(name: &str, x: i32, y: i32, w: u32, h: u32, scale: f64) -> MonitorGeom {
        MonitorGeom {
            name: Some(name.to_string()),
            work: PhysRect::new(x, y, w, h),
            scale,
        }
    }

    #[test]
    fn default_position_is_bottom_right_with_margin() {
        for s in SCALES {
            let work = PhysRect::new(0, 0, 1920, 1032);
            let (x, y) = default_position(work, s);
            let w = size_px(COLLAPSED_LOGICAL, s) as i32;
            let m = round_px(24.0 * s);
            assert_eq!(x, 1920 - w - m);
            assert_eq!(y, 1032 - w - m);
        }
    }

    #[test]
    fn restore_same_monitor_scales_with_current_scale() {
        // Saved at 1.0 on DISPLAY2 (right of primary); now DISPLAY2 runs at 1.5.
        let m1 = mon("D1", 0, 0, 1920, 1032, 1.0);
        let saved_on = mon("D2", 1920, 0, 1920, 1032, 1.0);
        let saved = to_saved((1920 + 300, 200), &saved_on);
        assert_eq!((saved.rel_x, saved.rel_y), (300.0, 200.0));
        let m2_now = mon("D2", 1920, 0, 2560, 1400, 1.5);
        let (pos, how) = resolve_position(Some(&saved), &[m1, m2_now], Some(0)).unwrap();
        assert_eq!(how, Placement::SameMonitor);
        assert_eq!(pos, (1920 + 450, 300));
    }

    #[test]
    fn restore_clamps_into_a_shrunken_work_area() {
        let before = mon("D1", 0, 0, 1920, 1080, 1.0);
        let saved = to_saved((1800, 1000), &before);
        let after = mon("D1", 0, 0, 1280, 720, 1.0);
        let (pos, how) = resolve_position(Some(&saved), &[after], Some(0)).unwrap();
        assert_eq!(how, Placement::SameMonitor);
        assert_eq!(
            pos,
            (
                1280 - COLLAPSED_LOGICAL as i32,
                720 - COLLAPSED_LOGICAL as i32
            )
        );
    }

    #[test]
    fn restore_with_negative_origin_monitor() {
        let left = mon("DL", -1920, -100, 1920, 1080, 1.0);
        let primary = mon("D1", 0, 0, 1920, 1032, 1.0);
        let saved = to_saved((-1500, 300), &left);
        let (pos, how) = resolve_position(Some(&saved), &[primary, left], Some(0)).unwrap();
        assert_eq!(how, Placement::SameMonitor);
        assert_eq!(pos, (-1500, 300));
    }

    #[test]
    fn unplugged_monitor_falls_back_to_default_on_primary() {
        let ext = mon("D2", 1920, 0, 1920, 1080, 1.0);
        let saved = to_saved((2500, 300), &ext);
        let only = mon("D1", 0, 0, 1920, 1032, 1.25);
        let (pos, how) =
            resolve_position(Some(&saved), std::slice::from_ref(&only), Some(0)).unwrap();
        assert_eq!(how, Placement::Default);
        assert_eq!(pos, default_position(only.work, 1.25));
    }

    #[test]
    fn renamed_monitor_uses_absolute_spot_when_still_on_screen() {
        let old = mon("OLD", 0, 0, 1920, 1032, 1.0);
        let saved = to_saved((600, 400), &old);
        let renamed = mon("NEW", 0, 0, 1920, 1032, 1.0);
        let (pos, how) = resolve_position(Some(&saved), &[renamed], Some(0)).unwrap();
        assert_eq!(how, Placement::Absolute);
        assert_eq!(pos, (600, 400));
    }

    #[test]
    fn no_saved_file_uses_default_and_no_monitors_returns_none() {
        let m = mon("D1", 0, 0, 1920, 1032, 1.0);
        let (_, how) = resolve_position(None, &[m], Some(0)).unwrap();
        assert_eq!(how, Placement::Default);
        assert!(resolve_position(None, &[], None).is_none());
    }

    #[test]
    fn corrupt_saved_values_are_ignored() {
        let m = mon("D1", 0, 0, 1920, 1032, 1.0);
        let mut saved = to_saved((100, 100), &m);
        saved.x = f64::NAN;
        let (_, how) = resolve_position(Some(&saved), std::slice::from_ref(&m), Some(0)).unwrap();
        assert_eq!(how, Placement::Default);
        let mut saved = to_saved((100, 100), &m);
        saved.version = 99;
        let (_, how) = resolve_position(Some(&saved), &[m], Some(0)).unwrap();
        assert_eq!(how, Placement::Default);
    }

    #[test]
    fn saved_position_json_round_trip() {
        let m = mon("D1", -1920, 0, 1920, 1080, 1.25);
        let s = to_saved((-1500, 300), &m);
        let json = serde_json::to_string(&s).unwrap();
        let back: SavedPosition = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    /// Centre of a window of `edge` logical px whose top-left is `pos` physical px, at `scale`.
    fn centre_of(pos: (i32, i32), edge: f64, scale: f64) -> (f64, f64) {
        PhysRect::new(pos.0, pos.1, size_px(edge, scale), size_px(edge, scale)).center()
    }

    #[test]
    fn v1_saved_position_keeps_the_sprite_centre() {
        // A file written by the 96 px build: top-left (1000, 500) on D2 at several scales.
        for s in SCALES {
            let m1 = mon("D1", 0, 0, 1920, 1032, 1.0);
            let m2 = mon("D2", 1920, 0, 3840, 2100, s);
            let old_pos = (1920 + round_px(300.0 * s), round_px(500.0 * s));
            let mut saved = to_saved(old_pos, &m2);
            saved.version = 1; // what the old build wrote
            let (pos, how) =
                resolve_position(Some(&saved), &[m1.clone(), m2.clone()], Some(0)).unwrap();
            assert_eq!(how, Placement::SameMonitor);
            approx(
                centre_of(pos, COLLAPSED_LOGICAL, s),
                centre_of(old_pos, LEGACY_V1_COLLAPSED_LOGICAL, s),
            );
            // renamed monitor: the absolute path must keep the centre too
            let renamed = mon("NEW", 1920, 0, 3840, 2100, s);
            let (pos, how) = resolve_position(Some(&saved), &[m1, renamed], Some(0)).unwrap();
            assert_eq!(how, Placement::Absolute);
            approx(
                centre_of(pos, COLLAPSED_LOGICAL, s),
                centre_of(old_pos, LEGACY_V1_COLLAPSED_LOGICAL, s),
            );
        }
    }

    #[test]
    fn migrate_saved_versions() {
        let m = mon("D1", 0, 0, 1920, 1032, 1.0);
        let mut v1 = to_saved((200, 300), &m);
        v1.version = 1;
        let up = migrate_saved(&v1).unwrap();
        assert_eq!(up.version, SAVED_POSITION_VERSION);
        let shift = (COLLAPSED_LOGICAL - LEGACY_V1_COLLAPSED_LOGICAL) / 2.0;
        assert_eq!((up.x, up.y), (200.0 - shift, 300.0 - shift));
        assert_eq!((up.rel_x, up.rel_y), (200.0 - shift, 300.0 - shift));
        // current records pass through unchanged, unknown ones are refused
        let cur = to_saved((200, 300), &m);
        assert_eq!(migrate_saved(&cur).unwrap(), cur);
        let mut future = cur;
        future.version = 3;
        assert!(migrate_saved(&future).is_none());
        // migrating twice is not possible: the result is version 2 and is left alone
        assert_eq!(migrate_saved(&up).unwrap(), up);
    }

    /// Reads `export const NAME = <number>;` from the frontend layout file.
    fn ts_const(src: &str, name: &str) -> f64 {
        let pat = format!("export const {name} = ");
        let line = src
            .lines()
            .find(|l| l.starts_with(&pat))
            .unwrap_or_else(|| panic!("{name} not found in layout.ts"));
        line[pat.len()..]
            .trim_end_matches(';')
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("{name} is not a plain number"))
    }

    /// The numbers exist twice (TypeScript, Rust) plus the initial window in tauri.conf.json:
    /// they must be equal, and the wheel must fit the expanded window.
    #[test]
    fn constants_match_frontend() {
        let ts = include_str!("../../src/features/floating/layout.ts");
        assert_eq!(ts_const(ts, "FLOATING_COLLAPSED_PX"), COLLAPSED_LOGICAL);
        assert_eq!(ts_const(ts, "FLOATING_EXPANDED_PX"), EXPANDED_LOGICAL);
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let win = conf["app"]["windows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["label"] == "floating")
            .expect("floating window in tauri.conf.json");
        assert_eq!(win["width"].as_f64(), Some(COLLAPSED_LOGICAL));
        assert_eq!(win["height"].as_f64(), Some(COLLAPSED_LOGICAL));
        // sprite fits the collapsed window with room for tilt and float; wheel items fit the expanded one
        let sprite = ts_const(ts, "FLOATING_SPRITE_PX");
        assert!(COLLAPSED_LOGICAL - sprite >= 16.0);
        let reach_x =
            0.866_025_4 * ts_const(ts, "WHEEL_RX").round() + ts_const(ts, "WHEEL_ITEM_W") / 2.0;
        let reach_y = ts_const(ts, "WHEEL_RY") + ts_const(ts, "WHEEL_ITEM_H") / 2.0;
        assert!(reach_x.max(reach_y) + 16.0 <= EXPANDED_LOGICAL / 2.0);
    }
}
