//! The CPU / RAM / network read-out drawn over the Windows taskbar.
//!
//! A small layered, click-through window sits just left of the clock/tray and
//! shows the same line as the top bar, right-aligned. It never polls: the
//! existing 3 s stats tick calls `render_taskbar`, which also re-places the
//! window, so a taskbar that moved, changed size or slid away is followed on
//! that tick (and at once on a theme or display change).

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::app::*;
use crate::gfx::*;

/// Logical width of the read-out: the same roomy line the top bar uses.
pub const READOUT_W: f32 = 254.0;
/// Logical height: the taskbar is ~40 px at 100%, so this centres with padding.
pub const READOUT_H: f32 = 22.0;
/// Gap between the read-out and the clock/tray.
const GAP: f32 = 10.0;
/// Tray + clock width to assume when the tray window cannot be found.
const TRAY_EST: f32 = 205.0;

pub struct TaskbarReadout {
    pub hwnd: HWND,
    gfx: Gfx,
    /// Theme the current pixels were drawn with.
    pub light: bool,
    /// Whether the window is currently shown.
    pub shown: bool,
}

impl TaskbarReadout {
    pub fn new(hwnd: HWND, scale: f32) -> Option<TaskbarReadout> {
        let gfx = Gfx::new((READOUT_W * scale).ceil() as i32 + 2, (READOUT_H * scale).ceil() as i32 + 2, scale).ok()?;
        Some(TaskbarReadout {
            hwnd,
            gfx,
            light: crate::sys::light_theme(),
            shown: false,
        })
    }
}

/// Top-left of the read-out window in screen pixels.
#[derive(Clone, Copy)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
}

/// Where the read-out should sit for the monitor `mon`, or `None` when that
/// monitor has no horizontal taskbar on screen (auto-hidden, vertical, or none).
pub fn place(mon: &RECT, scale: f32) -> Option<Placement> {
    let (bar, rect) = unsafe { find_taskbar(mon) }?;
    let vis = RECT {
        left: rect.left.max(mon.left),
        top: rect.top.max(mon.top),
        right: rect.right.min(mon.right),
        bottom: rect.bottom.min(mon.bottom),
    };
    let (vw, vh) = (vis.right - vis.left, vis.bottom - vis.top);
    let w = (READOUT_W * scale).round() as i32;
    let h = (READOUT_H * scale).round() as i32;
    // 2 px of a taskbar means it is auto-hidden and has slid away; a taskbar
    // taller than it is wide is a vertical one, which this line cannot fit.
    if vh < (8.0 * scale).round() as i32 || vw < vh * 2 || vw < w + 16 {
        return None;
    }
    let anchor = unsafe { tray_left(bar, vis) }.unwrap_or(vis.right - (TRAY_EST * scale).round() as i32);
    let x = (anchor - (GAP * scale).round() as i32 - w).max(vis.left + (4.0 * scale) as i32);
    let y = vis.top + ((vh - h) / 2).max(0);
    Some(Placement { x, y })
}

unsafe fn find_taskbar(mon: &RECT) -> Option<(HWND, RECT)> {
    let mut v: Vec<HWND> = Vec::new();
    let _ = EnumWindows(Some(enum_taskbars), LPARAM(&mut v as *mut _ as isize));
    let mut best: Option<(i64, HWND, RECT)> = None;
    for h in v {
        let mut r = RECT::default();
        if GetWindowRect(h, &mut r).is_err() {
            continue;
        }
        let area = overlap(&r, mon);
        if area > 0 && best.as_ref().map(|b| area > b.0).unwrap_or(true) {
            best = Some((area, h, r));
        }
    }
    best.map(|(_, h, r)| (h, r))
}

unsafe extern "system" fn enum_taskbars(h: HWND, lp: LPARAM) -> BOOL {
    let v = &mut *(lp.0 as *mut Vec<HWND>);
    if matches!(crate::sys::class_of(h).as_str(), "Shell_TrayWnd" | "Shell_SecondaryTrayWnd") {
        v.push(h);
    }
    BOOL(1)
}

/// Left edge of the clock/tray block, i.e. where the read-out should end.
unsafe fn tray_left(bar: HWND, vis: RECT) -> Option<i32> {
    let t = FindWindowExW(Some(bar), Option::<HWND>::None, w!("TrayNotifyWnd"), PCWSTR::null()).ok()?;
    let mut r = RECT::default();
    GetWindowRect(t, &mut r).ok()?;
    (r.left > vis.left && r.left < vis.right).then_some(r.left)
}

fn overlap(a: &RECT, b: &RECT) -> i64 {
    let x = (a.right.min(b.right) - a.left.max(b.left)).max(0) as i64;
    let y = (a.bottom.min(b.bottom) - a.top.max(b.top)).max(0) as i64;
    x * y
}

impl App {
    pub fn is_taskbar(&self, h: HWND) -> bool {
        self.taskbar.as_ref().map(|t| t.hwnd == h).unwrap_or(false)
    }

    /// The taskbar read-out is on and the bar/notch are not hidden by full screen or a tuck.
    pub fn taskbar_visible(&self) -> bool {
        self.cfg.stats_taskbar && !self.hidden
    }

    pub fn rebuild_taskbar(&mut self) {
        let Some(old) = self.taskbar.take() else { return };
        let shown = old.shown;
        let mut nt = TaskbarReadout::new(old.hwnd, self.scale);
        if let Some(n) = nt.as_mut() {
            n.shown = shown;
        }
        self.taskbar = nt;
    }

    /// Show or hide the read-out to match the settings, then repaint it.
    pub fn sync_taskbar(&mut self) {
        if !self.taskbar_visible() {
            self.hide_taskbar();
            return;
        }
        self.render_taskbar();
        self.raise_windows();
    }

    pub fn render_taskbar(&mut self) {
        if !self.taskbar_visible() {
            self.hide_taskbar();
            return;
        }
        let scale = self.scale;
        let Some(stats) = self.stats else {
            self.hide_taskbar();
            return;
        };
        let Some(p) = place(&self.mon, scale) else {
            self.hide_taskbar();
            return;
        };
        let light = crate::sys::light_theme();
        let Some(t) = self.taskbar.as_mut() else { return };
        t.light = light;
        let th = crate::bar::theme(light);
        let pw = (READOUT_W * scale).round() as i32;
        let ph = (READOUT_H * scale).round() as i32;
        let g = &t.gfx;
        if g.begin(pw, ph).is_err() {
            return;
        }
        let text = format!(
            "CPU {}% RAM {}%   \u{2193} {} \u{2191} {}",
            stats.cpu,
            stats.mem,
            crate::ui::rate_short(stats.rx),
            crate::ui::rate_short(stats.tx)
        );
        g.text(&text, Rect::new(0.0, 0.0, READOUT_W, READOUT_H), &g.fonts.menu, th.t1, Align::Right);
        let _ = g.end();
        let _ = g.present(t.hwnd, p.x, p.y, pw, ph);
        if !t.shown {
            unsafe {
                let _ = ShowWindow(t.hwnd, SW_SHOWNOACTIVATE);
            }
            t.shown = true;
        }
    }

    pub fn hide_taskbar(&mut self) {
        if let Some(t) = &mut self.taskbar {
            if t.shown {
                unsafe {
                    let _ = ShowWindow(t.hwnd, SW_HIDE);
                }
                t.shown = false;
            }
        }
    }

    /// Tray: draw the read-out over the Windows taskbar instead of the top bar.
    pub fn toggle_stats_taskbar(&mut self) {
        self.cfg.stats_taskbar = !self.cfg.stats_taskbar;
        crate::store::save_config(&self.cfg);
        self.sync_stats();
        self.sync_taskbar();
        self.render_bar();
    }
}
