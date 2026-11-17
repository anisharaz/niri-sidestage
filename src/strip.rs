//! The strip, drawn as layer-shell windows on the left edge of each monitor. It reserves no
//! screen space. A transparent trigger strip lives in niri's outer gap; touching it slides the
//! tile panel out.

use crate::niri::{self, OutputView, Snapshot, Tile, WsView};
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use gtk4 as gtk;
use gtk4_layer_shell::{Edge, Layer, LayerShell};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// Width of the invisible trigger strip. It sits in niri's outer gap (`gaps 5`), so it never
/// covers a window. Override with NIRI_SIDESTAGE_GAP if your gap differs.
fn gap() -> i32 {
    std::env::var("NIRI_SIDESTAGE_GAP").ok().and_then(|v| v.parse().ok()).unwrap_or(5)
}
const HANDLE_HEIGHT: i32 = 120;
const PANEL_WIDTH: i32 = 152;
const HIDE_DELAY: Duration = Duration::from_millis(250);

const CSS: &str = "
window.stage-strip, window.stage-strip.background,
window.stage-trigger, window.stage-trigger.background { background: transparent; }
window.stage-strip fixed { background: transparent; }
.handle { background: alpha(#4c8dff, 0.55); border-radius: 0 2px 2px 0; min-width: 2px; }
.strip-box { background: #15151b; padding: 14px 10px; border-radius: 0 18px 18px 0; }
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

const SLIDE_MS: f64 = 170.0;

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
    view: RefCell<OutputView>,
}

impl Panel {
    fn apply(&self) {
        let x = -(self.width.get() as f64) * (1.0 - self.p.get());
        self.holder.move_(&self.content, x, 0.0);
        self.update_region();
    }

    fn update_region(&self) {
        let Some(surface) = self.win.surface() else { return };
        let g = gap();
        // The gap strip always spans the whole height (the compositor clips it to the surface),
        // so it is never wrong, even before the first configure tells us the real size.
        let region = gtk::cairo::Region::create_rectangle(&gtk::cairo::RectangleInt::new(0, 0, g, 32_768));
        let shown = (self.width.get() as f64 * self.p.get()).ceil() as i32;
        if shown > 0 {
            // Use the panel's real position instead of recomputing the vertical centring.
            if let Some(b) = self.holder.compute_bounds(&self.win) {
                let _ = region.union_rectangle(&gtk::cairo::RectangleInt::new(
                    g,
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

        if ws_mode {
            for ws in view.workspaces.iter().filter(|w| w.active || !w.tiles.is_empty()) {
                self.content.append(&ws_block(ws, self));
            }
        } else {
            for t in view.active() {
                self.content.append(&tile(t, self));
            }
        }
        self.relayout();
    }

    /// Something was chosen: get out of the way, and go back to the plain window list.
    fn picked(self: &Rc<Self>) {
        if !self.pinned {
            self.close();
        }
        if self.ws_mode.get() {
            self.ws_mode.set(false);
            // Swap the content back once the panel has slid away (immediately when pinned).
            let me = self.clone();
            let delay = if self.pinned { 0 } else { SLIDE_MS as u64 + 60 };
            glib::timeout_add_local_once(Duration::from_millis(delay), move || me.rebuild());
        }
    }

    /// Re-measure after the tiles changed so the window matches the content.
    fn relayout(&self) {
        let min_h = if self.ws_mode.get() { self.win_height.get().max(-1) } else { -1 };
        self.content.set_size_request(PANEL_WIDTH, min_h);
        let (_, w, _, _) = self.content.measure(gtk::Orientation::Horizontal, -1);
        let (_, h, _, _) = self.content.measure(gtk::Orientation::Vertical, w);
        if !self.ws_mode.get() {
            self.win_height.set(h);
        }
        self.width.set(w);
        self.height.set(h);
        self.holder.set_size_request(w, h);
        self.root.set_size_request(gap() + w, -1);
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
        let dur = SLIDE_MS * 1000.0 * (target - from).abs();
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

    fn close(self: &Rc<Self>) {
        self.animate_to(0.0);
    }

    fn hide_now(&self) {
        if let Some(id) = self.tick.borrow_mut().take() {
            id.remove();
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

    let app2 = app.clone();
    glib::timeout_add_local(Duration::from_millis(60), move || {
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

fn schedule_hide(timer: &Timer, panel: &Rc<Panel>) {
    cancel(timer);
    let (t, p) = (timer.clone(), panel.clone());
    let id = glib::timeout_add_local_once(HIDE_DELAY, move || {
        t.borrow_mut().take();
        p.close();
    });
    *timer.borrow_mut() = Some(id);
}

fn make_panel(app: &gtk::Application, mon: &gtk::gdk::Monitor, pinned: bool) -> Rc<Panel> {
    let win = gtk::ApplicationWindow::new(app);
    win.add_css_class("stage-strip");
    win.init_layer_shell();
    win.set_layer(Layer::Top);
    win.set_monitor(Some(mon));
    win.set_namespace(Some("niri-sidestage"));
    win.set_exclusive_zone(0);
    for e in [Edge::Left, Edge::Top, Edge::Bottom] {
        win.set_anchor(e, true);
    }

    let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
    content.add_css_class("strip-box");
    // Same width in both views, so switching between them never makes the panel jump.
    content.set_size_request(PANEL_WIDTH, -1);
    let holder = gtk::Fixed::new();
    holder.set_overflow(gtk::Overflow::Hidden);
    holder.put(&content, 0.0, 0.0);
    holder.set_halign(gtk::Align::Start);
    holder.set_valign(gtk::Align::Center);
    holder.set_margin_start(gap());

    // A small marker in the gap shows where to point.
    let handle = gtk::Box::new(gtk::Orientation::Vertical, 0);
    handle.add_css_class("handle");
    handle.set_size_request(2, HANDLE_HEIGHT);
    handle.set_halign(gtk::Align::Start);
    handle.set_valign(gtk::Align::Center);

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
        ws_mode: Cell::new(std::env::var_os("NIRI_SIDESTAGE_START_WS").is_some()),
        drawn_ws_mode: Cell::new(false),
        win_height: Cell::new(0),
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
        let hide_timer: Timer = Rc::new(RefCell::new(None));
        let motion = gtk::EventControllerMotion::new();
        let g = gap() as f64;
        {
            // Touching the gap strip opens the panel at once. Entering through the panel area
            // while it is sliding closed must not reopen it.
            let (h, p) = (hide_timer.clone(), panel.clone());
            motion.connect_enter(move |_, x, _| {
                dbg_ev("enter", x);
                cancel(&h);
                if x < g + 1.0 && !p.is_open() {
                    p.open();
                }
            });
        }
        {
            let (h, p) = (hide_timer.clone(), panel.clone());
            motion.connect_motion(move |_, x, _| {
                dbg_ev("motion", x);
                cancel(&h);
                if x < g + 1.0 && !p.is_open() {
                    p.open();
                }
            });
        }
        {
            let (h, p) = (hide_timer.clone(), panel.clone());
            motion.connect_leave(move |_| {
                dbg_ev("leave", -1.0);
                schedule_hide(&h, &p)
            });
        }
        panel.win.add_controller(motion);
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
    found.unwrap_or_else(|| gio::ThemedIcon::new(&want).upcast())
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
    btn.connect_clicked(move |_| {
        niri::focus(id);
        panel.picked();
    });
    btn
}

/// One workspace in the breakdown: a heading that switches to it, and its windows as icons.
fn ws_block(ws: &WsView, panel: &Rc<Panel>) -> gtk::Box {
    let block = gtk::Box::new(gtk::Orientation::Vertical, 4);

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
    let (id, panel) = (t.id, panel.clone());
    btn.connect_clicked(move |_| {
        niri::focus(id);
        panel.picked();
    });
    btn
}
