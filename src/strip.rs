//! The strip, drawn as layer-shell windows on the left edge of each monitor. It reserves no
//! screen space. A transparent trigger strip lives in niri's outer gap; touching it slides the
//! tile panel out.

use crate::config::{self, Config, Side};
use crate::control::{self, Act, Cmd};
use crate::niri::{self, OutputView, Snapshot, Tile, WsView};
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

fn cfg() -> &'static Config {
    config::get()
}

/// Width of the invisible trigger strip. It sits in niri's outer gap, so it never covers a window.
fn gap() -> i32 {
    cfg().gap
}

fn right() -> bool {
    cfg().side == Side::Right
}

const CSS: &str = "
window.stage-strip, window.stage-strip.background,
window.stage-trigger, window.stage-trigger.background { background: transparent; }
window.stage-strip fixed { background: transparent; }
.handle { background: alpha(#4c8dff, 0.55); border-radius: 0 2px 2px 0; min-width: 2px; }
.handle.right { border-radius: 2px 0 0 2px; }
.strip-box { background: #15151b; padding: 14px 10px; border-radius: 0 18px 18px 0; }
.strip-box.right { border-radius: 18px 0 0 18px; }
.strip-box scrolledwindow, .strip-box scrolledwindow viewport { background: transparent; }
button:focus-visible { outline: 2px solid #4c8dff; outline-offset: -2px; }
.ws-block.drop { background: alpha(#4c8dff, 0.18); border-radius: 10px; }
button.tile {
    background: alpha(white, 0.07);
    border: 1px solid alpha(white, 0.10);
    border-radius: 14px;
    padding: 10px 6px;
    color: #e8e8ee;
    box-shadow: none;
    min-width: 96px;
}
button.tile:hover { background: alpha(white, 0.18); }
button.tile.current { border-color: #4c8dff; background: alpha(#4c8dff, 0.20); }
button.tile.urgent { border-color: #ff9f43; }
.tile-title { font-size: 11px; }
button.toggle {
    background: alpha(white, 0.07);
    border: 1px solid alpha(white, 0.10);
    border-radius: 12px;
    padding: 6px;
    color: #cfd2dc;
    font-size: 11px;
    box-shadow: none;
}
button.toggle:hover { background: alpha(white, 0.18); }
button.ws-head {
    background: transparent;
    border: none;
    box-shadow: none;
    color: #9aa0ae;
    font-size: 11px;
    font-weight: bold;
    padding: 2px 4px;
}
button.ws-head:hover { color: #e8e8ee; }
button.ws-head.active { color: #4c8dff; }
button.mini {
    background: alpha(white, 0.07);
    border: 1px solid alpha(white, 0.10);
    border-radius: 10px;
    padding: 4px;
    box-shadow: none;
    min-width: 0;
}
button.mini:hover { background: alpha(white, 0.18); }
button.mini.current { border-color: #4c8dff; background: alpha(#4c8dff, 0.20); }
button.mini.urgent { border-color: #ff9f43; }
";

type Timer = Rc<RefCell<Option<glib::SourceId>>>;

/// One layer window per monitor, anchored full height on the left edge and always mapped while
/// there is something to switch to. Which part of it reacts to the pointer is set with an input
/// region: only the gap strip when closed, the gap plus the visible panel when open. So the
/// pointer never hops between surfaces, which is what made the old two-window version flicker.
struct Panel {
    win: gtk::ApplicationWindow,
    root: gtk::Overlay,
    holder: gtk::Fixed,
    content: gtk::Box,
    width: Cell<i32>,
    height: Cell<i32>,
    /// 0.0 = slid out of view, 1.0 = fully shown.
    p: Cell<f64>,
    target: Cell<f64>,
    tick: RefCell<Option<gtk::TickCallbackId>>,
    pinned: bool,
    /// Showing the workspace breakdown instead of this workspace's windows.
    ws_mode: Cell<bool>,
    drawn_ws_mode: Cell<bool>,
    /// Height of the plain window list; the workspace view is never shorter, so the toggle
    /// button stays under the pointer when switching views.
    win_height: Cell<i32>,
    /// Height of this monitor; caps how tall the scrolling list may grow.
    mon_h: i32,
    /// Opened from the keyboard: stays open when the pointer leaves, and takes keyboard focus
    /// until Escape, a pick, or another toggle.
    sticky: Cell<bool>,
    /// A drag from this panel is in progress; the pointer is off the surface meanwhile.
    dragging: Cell<bool>,
    hide_timer: Timer,
    /// The marker in the gap; as tall as the trigger band.
    handle: gtk::Box,
    view: RefCell<OutputView>,
}

impl Panel {
    fn apply(&self) {
        let off = self.width.get() as f64 * (1.0 - self.p.get());
        self.holder.move_(&self.content, if right() { off } else { -off }, 0.0);
        self.update_region();
    }

    /// Width of the window: the gap plus the panel.
    fn win_w(&self) -> i32 {
        gap() + self.width.get()
    }

    /// Is `x` (in window coordinates) inside the gap strip at the screen edge?
    fn in_gap(&self, x: f64) -> bool {
        let g = gap() as f64;
        if right() { x > self.win_w() as f64 - g - 1.0 } else { x < g + 1.0 }
    }

    /// Vertical extent `(y, height)` of the edge trigger: the panel's height (at least
    /// `min_trigger`), centred like the panel. `None` until the window has a size.
    fn trigger_band(&self) -> Option<(i32, i32)> {
        let win_h = self.win.height();
        if win_h <= 0 {
            return None;
        }
        let h = self.height.get().max(cfg().min_trigger).min(win_h);
        Some(((win_h - h) / 2, h))
    }

    fn update_region(&self) {
        let Some(surface) = self.win.surface() else { return };
        let g = gap();
        // Before the first configure the window size is unknown; use a full-height strip then
        // (the compositor clips it to the surface), so the trigger is never missing.
        let (y, h) = self.trigger_band().unwrap_or((0, 32_768));
        let gap_x = if right() { self.win_w() - g } else { 0 };
        let region = gtk::cairo::Region::create_rectangle(&gtk::cairo::RectangleInt::new(gap_x, y, g, h));
        let shown = (self.width.get() as f64 * self.p.get()).ceil() as i32;
        if shown > 0 {
            // Use the panel's real position instead of recomputing the vertical centring.
            if let Some(b) = self.holder.compute_bounds(&self.win) {
                let x = if right() { self.win_w() - g - shown } else { g };
                let _ = region.union_rectangle(&gtk::cairo::RectangleInt::new(
                    x,
                    b.y().floor() as i32,
                    shown,
                    b.height().ceil() as i32,
                ));
            }
        }
        surface.set_input_region(Some(&region));
    }

    /// Redraw the panel content from `self.view`: a toggle button on top, then either this
    /// workspace's windows or the workspace breakdown.
    fn rebuild(self: &Rc<Self>) {
        dbg_ev(if self.ws_mode.get() { "rebuild-ws" } else { "rebuild-win" }, -3.0);
        while let Some(c) = self.content.first_child() {
            self.content.remove(&c);
        }
        let view = self.view.borrow().clone();
        let ws_mode = self.ws_mode.get();
        self.drawn_ws_mode.set(ws_mode);

        let toggle = gtk::Button::with_label(if ws_mode { "‹  Windows" } else { "Workspaces  ›" });
        toggle.add_css_class("toggle");
        {
            let me = self.clone();
            toggle.connect_clicked(move |_| {
                dbg_ev("toggle", -2.0);
                me.ws_mode.set(!me.ws_mode.get());
                // Rebuild after this click has finished, not while its button is being handled.
                let me2 = me.clone();
                glib::idle_add_local_once(move || me2.rebuild());
            });
        }
        self.content.append(&toggle);

        let list = gtk::Box::new(gtk::Orientation::Vertical, 10);
        if ws_mode {
            for ws in view.workspaces.iter().filter(|w| w.active || !w.tiles.is_empty()) {
                list.append(&ws_block(ws, self));
            }
        } else {
            for t in view.active() {
                list.append(&tile(t, self));
            }
        }
        // Long lists scroll instead of growing taller than the monitor.
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_propagate_natural_width(true);
        scroller.set_propagate_natural_height(true);
        scroller.set_max_content_height((self.mon_h - 140).max(200));
        scroller.set_vexpand(true);
        scroller.set_child(Some(&list));
        self.content.append(&scroller);
        self.relayout();
        if self.sticky.get() {
            self.focus_current();
        }
    }

    /// Something was chosen: get out of the way, and go back to the plain window list.
    fn picked(self: &Rc<Self>) {
        self.set_sticky(false);
        if !self.pinned {
            self.close();
        }
        if self.ws_mode.get() != start_ws() {
            self.ws_mode.set(start_ws());
            // Swap the content back to the default view once the panel has slid away (immediately when pinned).
            let me = self.clone();
            let delay = if self.pinned { 0 } else { cfg().slide_ms as u64 + 60 };
            glib::timeout_add_local_once(Duration::from_millis(delay), move || me.rebuild());
        }
    }

    /// Re-measure after the tiles changed so the window matches the content.
    fn relayout(&self) {
        let min_h = if self.ws_mode.get() { self.win_height.get().max(-1) } else { -1 };
        self.content.set_size_request(cfg().panel_width, min_h);
        let (_, w, _, _) = self.content.measure(gtk::Orientation::Horizontal, -1);
        let (hmin, h, _, _) = self.content.measure(gtk::Orientation::Vertical, w);
        // GtkFixed allocates children at their minimum size, and a scroller's minimum is far
        // below its natural height, so pin the content to the natural size.
        let _ = hmin;
        self.content.set_size_request(w, h);
        if !self.ws_mode.get() {
            self.win_height.set(h);
        }
        self.width.set(w);
        self.height.set(h);
        self.holder.set_size_request(w, h);
        self.root.set_size_request(gap() + w, -1);
        self.handle.set_size_request(2, h.max(cfg().min_trigger));
        self.apply();
    }

    fn is_open(&self) -> bool {
        self.target.get() >= 1.0
    }

    fn animate_to(self: &Rc<Self>, target: f64) {
        self.target.set(target);
        if let Some(id) = self.tick.borrow_mut().take() {
            id.remove();
        }
        let from = self.p.get();
        if (from - target).abs() < 1e-3 {
            self.p.set(target);
            self.apply();
            return;
        }
        let dur = cfg().slide_ms * 1000.0 * (target - from).abs();
        let start: Cell<Option<i64>> = Cell::new(None);
        let me = self.clone();
        let id = self.holder.add_tick_callback(move |_, clock| {
            let now = clock.frame_time();
            let t0 = start.get().unwrap_or_else(|| {
                start.set(Some(now));
                now
            });
            let frac = ((now - t0) as f64 / dur).min(1.0);
            let eased = 1.0 - (1.0 - frac).powi(3);
            me.p.set(from + (target - from) * eased);
            me.apply();
            if frac >= 1.0 {
                me.tick.borrow_mut().take();
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
        *self.tick.borrow_mut() = Some(id);
    }

    fn open(self: &Rc<Self>) {
        self.animate_to(1.0);
    }

    fn set_sticky(&self, on: bool) {
        self.sticky.set(on);
        self.win.set_keyboard_mode(if on { KeyboardMode::Exclusive } else { KeyboardMode::None });
    }

    /// Open from the keyboard (or a control command): stays until dismissed, takes the keyboard.
    fn open_sticky(self: &Rc<Self>) {
        if !self.win.is_visible() {
            return;
        }
        cancel(&self.hide_timer);
        self.set_sticky(true);
        self.open();
        self.focus_current();
    }

    /// Dismiss a keyboard-opened panel.
    fn release(self: &Rc<Self>) {
        self.set_sticky(false);
        if !self.pinned {
            self.close();
        }
    }

    /// Put keyboard focus on the focused window's button (or the first one).
    fn focus_current(&self) {
        fn walk(w: &gtk::Widget, cur: &mut Option<gtk::Widget>, first: &mut Option<gtk::Widget>) {
            if w.is::<gtk::Button>() && !w.has_css_class("toggle") {
                if first.is_none() {
                    *first = Some(w.clone());
                }
                if cur.is_none() && w.has_css_class("current") {
                    *cur = Some(w.clone());
                }
            }
            let mut c = w.first_child();
            while let Some(child) = c {
                walk(&child, cur, first);
                c = child.next_sibling();
            }
        }
        let (mut cur, mut first) = (None, None);
        walk(self.content.upcast_ref(), &mut cur, &mut first);
        if let Some(w) = cur.or(first) {
            glib::idle_add_local_once(move || {
                w.grab_focus();
            });
        }
    }

    /// Close shortly after the pointer left, unless something keeps the panel open.
    fn schedule_hide(self: &Rc<Self>) {
        cancel(&self.hide_timer);
        let p = self.clone();
        let id = glib::timeout_add_local_once(cfg().hide_delay, move || {
            p.hide_timer.borrow_mut().take();
            if !p.sticky.get() && !p.dragging.get() {
                p.close();
            }
        });
        *self.hide_timer.borrow_mut() = Some(id);
    }

    fn close(self: &Rc<Self>) {
        self.animate_to(0.0);
    }

    fn hide_now(&self) {
        if let Some(id) = self.tick.borrow_mut().take() {
            id.remove();
        }
        if self.sticky.get() {
            self.set_sticky(false);
        }
        self.target.set(0.0);
        self.p.set(0.0);
        self.apply();
        self.win.set_visible(false);
    }
}

struct Strip {
    panel: Rc<Panel>,
    /// What is currently drawn, so unchanged snapshots do not rebuild (and flicker) the tiles.
    last: RefCell<OutputView>,
}

type Strips = Rc<RefCell<HashMap<String, Strip>>>;

pub fn run(pinned: bool) {
    let app = gtk::Application::builder()
        .application_id("dev.anish.niri-sidestage")
        .build();
    app.connect_activate(move |app| build(app, pinned));
    app.run_with_args::<&str>(&[]);
}

fn build(app: &gtk::Application, pinned: bool) {
    let provider = gtk::CssProvider::new();
    provider.load_from_data(CSS);
    let display = gtk::gdk::Display::default().expect("no display");
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let strips: Strips = Rc::new(RefCell::new(HashMap::new()));
    let latest: Rc<RefCell<Snapshot>> = Rc::new(RefCell::new(Snapshot::new()));

    // Monitor hot-plug: drop the windows, they are rebuilt on the next render.
    {
        let (strips, latest, app) = (strips.clone(), latest.clone(), app.clone());
        display.monitors().connect_items_changed(move |_, _, _, _| {
            let old: Vec<Strip> = strips.borrow_mut().drain().map(|(_, s)| s).collect();
            for s in old {
                s.panel.win.close();
            }
            render(&app, &strips, &latest.borrow(), pinned);
        });
    }

    let (tx, rx) = mpsc::channel::<Snapshot>();
    thread::spawn(move || niri::watch(tx));

    let (ctx, crx) = mpsc::channel::<Cmd>();
    if let Err(e) = control::serve(ctx) {
        eprintln!("niri-sidestage: control socket unavailable: {e}");
    }

    let app2 = app.clone();
    glib::timeout_add_local(Duration::from_millis(60), move || {
        while let Ok(c) = crx.try_recv() {
            if let Some(s) = strips.borrow().get(&c.output) {
                let p = &s.panel;
                match c.act {
                    Act::Open => p.open_sticky(),
                    Act::Close => p.release(),
                    Act::Toggle if p.sticky.get() => p.release(),
                    Act::Toggle => p.open_sticky(),
                }
            }
        }
        let mut newest = None;
        while let Ok(s) = rx.try_recv() {
            newest = Some(s);
        }
        if let Some(s) = newest {
            render(&app2, &strips, &s, pinned);
            *latest.borrow_mut() = s;
        }
        glib::ControlFlow::Continue
    });
    // Keep the process alive while every strip window is hidden.
    std::mem::forget(app.hold());
}

fn render(app: &gtk::Application, strips: &Strips, snap: &Snapshot, pinned: bool) {
    let Some(display) = gtk::gdk::Display::default() else { return };
    let monitors = display.monitors();
    for i in 0..monitors.n_items() {
        let Some(mon) = monitors.item(i).and_downcast::<gtk::gdk::Monitor>() else { continue };
        let Some(conn) = mon.connector().map(|c| c.to_string()) else { continue };
        let view = snap.get(&conn).cloned().unwrap_or_default();

        let mut map = strips.borrow_mut();
        let strip = map.entry(conn).or_insert_with(|| Strip {
            panel: make_panel(app, &mon, pinned),
            last: RefCell::new(OutputView::default()),
        });
        let panel = &strip.panel;

        if *strip.last.borrow() != view || panel.drawn_ws_mode.get() != panel.ws_mode.get() {
            *panel.view.borrow_mut() = view.clone();
            panel.rebuild();
            *strip.last.borrow_mut() = view.clone();
        }

        // With a single window on the whole monitor there is nothing to switch to.
        if view.total() <= 1 {
            panel.hide_now();
        } else {
            panel.win.set_visible(true);
            if pinned && panel.p.get() < 1.0 {
                panel.p.set(1.0);
                panel.target.set(1.0);
                panel.apply();
            }
        }
    }
}

fn dbg_ev(what: &str, x: f64) {
    if std::env::var_os("NIRI_SIDESTAGE_DEBUG").is_some() {
        let t = glib::monotonic_time() / 1000;
        eprintln!("{t:>10} ms  {what:<7} x={x:.1}");
    }
}

fn cancel(timer: &Timer) {
    if let Some(id) = timer.borrow_mut().take() {
        id.remove();
    }
}

fn make_panel(app: &gtk::Application, mon: &gtk::gdk::Monitor, pinned: bool) -> Rc<Panel> {
    let win = gtk::ApplicationWindow::new(app);
    win.add_css_class("stage-strip");
    win.init_layer_shell();
    win.set_layer(Layer::Top);
    win.set_monitor(Some(mon));
    win.set_namespace(Some("niri-sidestage"));
    win.set_exclusive_zone(0);
    let side = if right() { Edge::Right } else { Edge::Left };
    for e in [side, Edge::Top, Edge::Bottom] {
        win.set_anchor(e, true);
    }

    let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
    content.add_css_class("strip-box");
    // Same width in both views, so switching between them never makes the panel jump.
    content.set_size_request(cfg().panel_width, -1);
    let holder = gtk::Fixed::new();
    holder.set_overflow(gtk::Overflow::Hidden);
    holder.put(&content, 0.0, 0.0);
    holder.set_valign(gtk::Align::Center);
    if right() {
        content.add_css_class("right");
        holder.set_halign(gtk::Align::End);
        holder.set_margin_end(gap());
    } else {
        holder.set_halign(gtk::Align::Start);
        holder.set_margin_start(gap());
    }

    // A small marker in the gap shows where to point.
    let handle = gtk::Box::new(gtk::Orientation::Vertical, 0);
    handle.add_css_class("handle");
    handle.set_size_request(2, cfg().min_trigger);
    handle.set_halign(if right() { gtk::Align::End } else { gtk::Align::Start });
    handle.set_valign(gtk::Align::Center);
    if right() {
        handle.add_css_class("right");
    }

    let root = gtk::Overlay::new();
    root.set_child(Some(&holder));
    root.add_overlay(&handle);
    root.set_size_request(gap(), -1);
    win.set_child(Some(&root));

    let panel = Rc::new(Panel {
        win,
        root,
        holder,
        content,
        width: Cell::new(0),
        height: Cell::new(0),
        p: Cell::new(if pinned { 1.0 } else { 0.0 }),
        target: Cell::new(if pinned { 1.0 } else { 0.0 }),
        tick: RefCell::new(None),
        pinned,
        ws_mode: Cell::new(start_ws()),
        drawn_ws_mode: Cell::new(false),
        win_height: Cell::new(0),
        mon_h: mon.geometry().height(),
        sticky: Cell::new(false),
        dragging: Cell::new(false),
        hide_timer: Rc::new(RefCell::new(None)),
        handle,
        view: RefCell::new(OutputView::default()),
    });

    // Keep the input region in step with the surface size.
    {
        let p = panel.clone();
        panel.win.connect_realize(move |w| {
            if let Some(surface) = w.surface() {
                let p2 = p.clone();
                surface.connect_layout(move |_, _, _| p2.update_region());
            }
            p.update_region();
        });
    }

    if !pinned {
        let motion = gtk::EventControllerMotion::new();
        {
            // Touching the gap strip opens the panel at once. Entering through the panel area
            // while it is sliding closed must not reopen it.
            let p = panel.clone();
            motion.connect_enter(move |_, x, _| {
                dbg_ev("enter", x);
                cancel(&p.hide_timer);
                if p.in_gap(x) && !p.is_open() {
                    p.open();
                }
            });
        }
        {
            let p = panel.clone();
            motion.connect_motion(move |_, x, _| {
                dbg_ev("motion", x);
                cancel(&p.hide_timer);
                if p.in_gap(x) && !p.is_open() {
                    p.open();
                }
            });
        }
        {
            let p = panel.clone();
            motion.connect_leave(move |_| {
                dbg_ev("leave", -1.0);
                p.schedule_hide();
            });
        }
        panel.win.add_controller(motion);
    }

    // Escape dismisses a keyboard-opened panel.
    {
        let keys = gtk::EventControllerKey::new();
        let p = panel.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape && p.sticky.get() {
                p.release();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        panel.win.add_controller(keys);
    }

    panel
}

thread_local! {
    static APPS: Vec<gio::AppInfo> = gio::AppInfo::all();
}

/// Match a window's app_id to an installed desktop entry (by id, then by executable name).
fn icon_for(app_id: &str) -> gio::Icon {
    let want = app_id.to_lowercase();
    let found = APPS.with(|apps| {
        let by_id = apps.iter().find(|a| {
            a.id().is_some_and(|id| id.to_lowercase().trim_end_matches(".desktop") == want)
        });
        let by_exe = || {
            apps.iter().find(|a| {
                a.executable()
                    .as_deref()
                    .and_then(|p| p.file_name())
                    .is_some_and(|n| n.to_string_lossy().to_lowercase() == want)
            })
        };
        by_id.or_else(by_exe).and_then(|a| a.icon())
    });
    found.unwrap_or_else(|| gio::ThemedIcon::from_names(&[&want, "application-x-executable"]).upcast())
}

fn tile(t: &Tile, panel: &Rc<Panel>) -> gtk::Button {
    let inner = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let img = gtk::Image::from_gicon(&icon_for(&t.app_id));
    img.set_pixel_size(48);
    inner.append(&img);

    let label = if t.title.is_empty() { &t.app_id } else { &t.title };
    let title = gtk::Label::new(Some(label));
    title.add_css_class("tile-title");
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    title.set_max_width_chars(14);
    inner.append(&title);

    let btn = gtk::Button::new();
    btn.add_css_class("tile");
    if t.focused {
        btn.add_css_class("current");
    }
    if t.urgent {
        btn.add_css_class("urgent");
    }
    btn.set_child(Some(&inner));
    btn.set_tooltip_text(Some(label));
    let id = t.id;
    let panel = panel.clone();
    {
        let panel = panel.clone();
        btn.connect_clicked(move |_| {
            niri::focus(id);
            panel.picked();
        });
    }
    middle_click_closes(&btn, id);
    btn
}

/// Middle-click on a window's button closes that window.
fn middle_click_closes(btn: &gtk::Button, id: u64) {
    let click = gtk::GestureClick::builder().button(2).build();
    click.connect_released(move |g, _, _, _| {
        g.set_state(gtk::EventSequenceState::Claimed);
        niri::close(id);
    });
    btn.add_controller(click);
}

/// Does the strip open in the workspace breakdown? (`view` in the config, or the env override.)
fn start_ws() -> bool {
    std::env::var_os("NIRI_SIDESTAGE_START_WS").is_some() || cfg().view == crate::config::View::Workspaces
}

/// One workspace in the breakdown: a heading that switches to it, and its windows as icons.
fn ws_block(ws: &WsView, panel: &Rc<Panel>) -> gtk::Box {
    let block = gtk::Box::new(gtk::Orientation::Vertical, 4);
    block.add_css_class("ws-block");

    // Drop a window here to move it to this workspace.
    {
        let target = gtk::DropTarget::new(glib::Type::U64, gtk::gdk::DragAction::MOVE);
        let b = block.clone();
        target.connect_enter(move |_, _, _| {
            b.add_css_class("drop");
            gtk::gdk::DragAction::MOVE
        });
        let b = block.clone();
        target.connect_leave(move |_| b.remove_css_class("drop"));
        let (b, ws_id) = (block.clone(), ws.id);
        target.connect_drop(move |_, value, _, _| {
            b.remove_css_class("drop");
            match value.get::<u64>() {
                Ok(win) => {
                    niri::move_window(win, ws_id);
                    true
                }
                Err(_) => false,
            }
        });
        block.add_controller(target);
    }

    let name = ws.name.clone().unwrap_or_else(|| ws.idx.to_string());
    let head = gtk::Button::with_label(&format!("Workspace {name}  ·  {}", ws.tiles.len()));
    head.add_css_class("ws-head");
    if ws.active {
        head.add_css_class("active");
    }
    if let Some(l) = head.child().and_downcast::<gtk::Label>() {
        l.set_xalign(0.0);
        l.set_ellipsize(gtk::pango::EllipsizeMode::End);
    }
    {
        let (id, panel) = (ws.id, panel.clone());
        head.connect_clicked(move |_| {
            niri::focus_workspace(id);
            panel.picked();
        });
    }
    block.append(&head);

    // Fixed rows of three icons: predictable to measure, unlike a flow layout.
    for row in ws.tiles.chunks(3) {
        let r = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        for t in row {
            r.append(&mini(t, panel));
        }
        block.append(&r);
    }
    block
}

fn mini(t: &Tile, panel: &Rc<Panel>) -> gtk::Button {
    let img = gtk::Image::from_gicon(&icon_for(&t.app_id));
    img.set_pixel_size(26);
    let btn = gtk::Button::new();
    btn.add_css_class("mini");
    if t.focused {
        btn.add_css_class("current");
    }
    if t.urgent {
        btn.add_css_class("urgent");
    }
    btn.set_child(Some(&img));
    btn.set_tooltip_text(Some(if t.title.is_empty() { &t.app_id } else { &t.title }));
    let id = t.id;
    {
        let panel = panel.clone();
        btn.connect_clicked(move |_| {
            niri::focus(id);
            panel.picked();
        });
    }
    middle_click_closes(&btn, id);

    // Drag onto another workspace's block to move the window there. The pointer leaves the
    // surface during a drag, so the panel must not auto-hide until it ends.
    let src = gtk::DragSource::new();
    src.set_actions(gtk::gdk::DragAction::MOVE);
    src.connect_prepare(move |_, _, _| Some(gtk::gdk::ContentProvider::for_value(&id.to_value())));
    {
        let (panel, btn) = (panel.clone(), btn.clone());
        src.connect_drag_begin(move |s, _| {
            panel.dragging.set(true);
            cancel(&panel.hide_timer);
            s.set_icon(Some(&gtk::WidgetPaintable::new(Some(&btn))), 0, 0);
        });
    }
    {
        let panel = panel.clone();
        src.connect_drag_end(move |_, _, _| {
            panel.dragging.set(false);
            if !panel.sticky.get() {
                panel.schedule_hide();
            }
        });
    }
    btn.add_controller(src);
    btn
}
