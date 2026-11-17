//! Talks to niri: turns its event stream into per-output workspace/window lists, and sends
//! focus requests.

use niri_ipc::socket::Socket;
use niri_ipc::state::{EventStreamState, EventStreamStatePart};
use niri_ipc::{Action, Request, WorkspaceReferenceArg};
use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    pub id: u64,
    pub title: String,
    pub app_id: String,
    pub focused: bool,
    pub urgent: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WsView {
    pub id: u64,
    pub idx: u8,
    pub name: Option<String>,
    pub active: bool,
    /// In layout order (column, then row), floating windows last.
    pub tiles: Vec<Tile>,
}

/// Everything shown for one monitor: its workspaces (in niri's order), each with its windows.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct OutputView {
    pub workspaces: Vec<WsView>,
}

impl OutputView {
    /// Windows on the workspace currently shown on this monitor.
    pub fn active(&self) -> &[Tile] {
        self.workspaces
            .iter()
            .find(|w| w.active)
            .map(|w| w.tiles.as_slice())
            .unwrap_or(&[])
    }

    pub fn total(&self) -> usize {
        self.workspaces.iter().map(|w| w.tiles.len()).sum()
    }
}

/// Output name -> that monitor's workspaces and windows.
pub type Snapshot = HashMap<String, OutputView>;

fn snapshot(state: &EventStreamState) -> Snapshot {
    let mut out = Snapshot::new();
    for ws in state.workspaces.workspaces.values() {
        let Some(output) = ws.output.as_ref() else { continue };
        let mut wins: Vec<_> = state
            .windows
            .windows
            .values()
            .filter(|w| w.workspace_id == Some(ws.id))
            .collect();
        // Follow the layout: tiled windows by (column, row), floating ones after, by open order.
        // Focus changes never reorder anything; moving a window does.
        wins.sort_by_key(|w| {
            let (col, row) = w.layout.pos_in_scrolling_layout.unwrap_or((usize::MAX, usize::MAX));
            (col, row, w.id)
        });
        out.entry(output.clone()).or_default().workspaces.push(WsView {
            id: ws.id,
            idx: ws.idx,
            name: ws.name.clone(),
            active: ws.is_active,
            tiles: wins
                .into_iter()
                .map(|w| Tile {
                    id: w.id,
                    title: w.title.clone().unwrap_or_default(),
                    app_id: w.app_id.clone().unwrap_or_default(),
                    focused: w.is_focused,
                    urgent: w.is_urgent,
                })
                .collect(),
        });
    }
    for view in out.values_mut() {
        view.workspaces.sort_by_key(|w| w.idx);
    }
    out
}

/// Follow niri's event stream forever, sending a fresh snapshot whenever it changes.
pub fn watch(tx: Sender<Snapshot>) {
    loop {
        let _ = follow(&tx);
        thread::sleep(Duration::from_secs(1));
    }
}

fn follow(tx: &Sender<Snapshot>) -> std::io::Result<()> {
    let mut sock = Socket::connect()?;
    if sock.send(Request::EventStream)?.is_err() {
        return Ok(());
    }
    let mut next = sock.read_events();
    let mut state = EventStreamState::default();
    let mut last = Snapshot::new();
    loop {
        state.apply(next()?);
        let snap = snapshot(&state);
        if snap != last {
            if tx.send(snap.clone()).is_err() {
                return Ok(());
            }
            last = snap;
        }
    }
}

fn action(a: Action) {
    thread::spawn(move || {
        if let Ok(mut s) = Socket::connect() {
            let _ = s.send(Request::Action(a));
        }
    });
}

pub fn focus(id: u64) {
    action(Action::FocusWindow { id });
}

pub fn focus_workspace(id: u64) {
    action(Action::FocusWorkspace { reference: WorkspaceReferenceArg::Id(id) });
}
