# niri-sidestage

A slide-out side strip for [niri](https://github.com/YaLTeR/niri) that lists the windows on your
current workspace, and shows a per-monitor breakdown of every workspace and its windows. Touch the
left edge of a monitor and it slides out; click a window to jump to it.

It started as an attempt to copy macOS Stage Manager. That version hid windows on parked
workspaces, which fought with niri's layout (widths, focus, workspace switching), so it was dropped.
Windows now stay exactly where niri put them and the strip is purely a switcher.

## Features

- **Per monitor.** Each monitor gets its own strip showing only its own windows and workspaces.
- **Edge trigger, no reserved space.** A transparent strip sits in niri's outer gap, so it never
  covers a window. The panel opens the moment the pointer touches the edge, and closes shortly after
  the pointer leaves or after you pick something. Windows are never resized by the strip.
- **Window list.** The windows on the workspace shown on that monitor, in layout order (column, then
  row; floating windows last). The focused window is highlighted and urgent ones get an accent.
  Focusing a window never reorders the list, only moving it does.
- **Workspace breakdown.** The "Workspaces" button switches the panel to that monitor's workspaces,
  each with its windows as icons. Click a window to focus it (niri switches workspace if needed), or
  click a heading to switch to that workspace. Empty workspaces are hidden unless active.
- **Hides when pointless.** A monitor with a single window in total shows no strip.
- Icons come from your installed desktop entries.

## Requirements

- niri **26.04**. The IPC crate is pinned to the matching `niri-ipc 26.4`; a different niri version
  may speak a different IPC protocol.
- A Rust toolchain (edition 2024).
- GTK 4 and [gtk4-layer-shell](https://github.com/wmww/gtk4-layer-shell) system libraries.
  On Arch: `sudo pacman -S gtk4 gtk4-layer-shell`.

## Build and install

```sh
git clone git@github.com:anisharaz/niri-sidestage.git
cd niri-sidestage
cargo build --release
install -Dm755 target/release/niri-sidestage ~/.local/bin/niri-sidestage
```

Start it with niri by adding this to your niri config:

```kdl
spawn-at-startup "niri-sidestage"
```

## Usage

```
niri-sidestage            run the strip (auto-hides, slides out when the pointer hits the left edge)
niri-sidestage --pinned   keep the strip open
```

Move the pointer to the left edge of a monitor. The thin blue marker in the middle of the edge shows
where the strip lives, but the whole height of the edge triggers it.

## Configuration

There is no config file. Environment variables:

| Variable | Default | Meaning |
|---|---|---|
| `NIRI_SIDESTAGE_GAP` | `5` | Width in pixels of the edge trigger. Set it to your niri `gaps` value so the trigger stays inside the gap and never covers a window. |
| `NIRI_SIDESTAGE_DEBUG` | unset | Print pointer enter/leave/motion events to stderr. |
| `NIRI_SIDESTAGE_START_WS` | unset | Start in the workspace breakdown view. |

For example, in your niri config: `spawn-at-startup "env" "NIRI_SIDESTAGE_GAP=8" "niri-sidestage"`.

## How it works

One process, no daemon.

- It follows niri's event stream (`niri-ipc`) and keeps a snapshot of workspaces and windows per
  output. Clicks send `FocusWindow` or `FocusWorkspace` actions back over the IPC socket.
- Per monitor there is one full-height layer-shell window anchored to the left edge. An input region
  keeps only the gap strip interactive while closed, and the gap plus the visible panel while open.
  Because it is a single surface, the pointer never hands off between windows, which avoids
  flicker from enter/leave races.
- The panel's window has a fixed size. Opening and closing slides the content inside it, driven by
  the frame clock. Resizing a layer-shell window on every frame lags behind the compositor and tears.

## Limitations

- The open panel covers roughly the leftmost 160px of the window beneath it. It reserves no space
  on purpose, so it cannot avoid that.
- No window thumbnails, only app icons and titles.
- The workspace breakdown does not scroll; a very large number of workspaces will make the panel
  taller than the screen.
- Tested on niri 26.04 on Arch Linux with two 1080p monitors at scale 1. Other setups, such as
  fractional scaling or a left-hand layout, are untested.
- niri's default top-left hot corner opens the overview. If you reach the edge by sliding up along
  it, you may brush that corner. You can disable it with `hot-corners { off }` in your niri config.

## License

No license has been chosen yet.
