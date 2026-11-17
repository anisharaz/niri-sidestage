# niri-sidestage

A slide-out side strip for [niri](https://github.com/YaLTeR/niri) that lists the windows on your
current workspace, and shows a per-monitor breakdown of every workspace and its windows. Touch the
edge of a monitor and it slides out; click a window to jump to it.

It started as an attempt to copy macOS Stage Manager. That version hid windows on parked
workspaces, which fought with niri's layout (widths, focus, workspace switching), so it was dropped.
Windows now stay exactly where niri put them and the strip is purely a switcher.

## Features

- **Per monitor.** Each monitor gets its own strip showing only its own windows and workspaces.
- **Edge trigger, no reserved space.** A transparent strip sits in niri's outer gap, so it never
  covers a window. The panel opens the moment the pointer touches the edge level with it, and closes
  shortly after the pointer leaves or after you pick something. Windows are never resized by the
  strip.
- **Window list.** The windows on the workspace shown on that monitor, in layout order (column, then
  row; floating windows last). The focused window is highlighted and urgent ones get an accent.
  Focusing a window never reorders the list, only moving it does. Long lists scroll.
- **Workspace breakdown.** The "Workspaces" button switches the panel to that monitor's workspaces,
  each with its windows as icons. Click a window to focus it (niri switches workspace if needed), or
  click a heading to switch to that workspace. Empty workspaces are hidden unless active.
- **Drag to move.** In the breakdown, drag a window's icon onto another workspace to move it there.
- **Middle-click to close** a window from its tile or icon. Hover for the full title.
- **Keyboard control.** `niri-sidestage toggle` opens the panel on the focused monitor and gives it
  the keyboard: arrows or Tab to move, Enter to pick, Escape to dismiss.
- **Left or right edge,** and the gap is detected from your niri config.
- **Hides when pointless.** A monitor with a single window in total shows no strip.
- Icons come from your installed desktop entries.

## Requirements

- niri **26.04**. The IPC crate is pinned to the matching `niri-ipc 26.4`; a different niri version
  may speak a different IPC protocol.
- GTK 4 and [gtk4-layer-shell](https://github.com/wmww/gtk4-layer-shell) 1.3 or newer.
  On Arch: `sudo pacman -S gtk4 gtk4-layer-shell`.
- To build from source, a Rust toolchain (edition 2024).

## Install

### From a release

Prebuilt x86_64 Linux binaries are on the
[releases page](https://github.com/anisharaz/niri-sidestage/releases). The install script downloads
the latest one, verifies its SHA-256 checksum, and puts it in `~/.local/bin`:

```sh
curl -fsSL https://github.com/anisharaz/niri-sidestage/releases/latest/download/install.sh | sh
```

You still need the GTK 4 and gtk4-layer-shell libraries installed (see Requirements); the script
warns if any are missing. The binary needs glibc 2.34 or newer. Set `NIRI_SIDESTAGE_VERSION=v0.2.0` to pin a version or `PREFIX=/some/dir` to change where it
installs, and run `sh install.sh uninstall` to remove it. If you would rather read the script
first, it is [`install.sh`](install.sh) in this repo.

### Arch Linux

A `PKGBUILD` is in [`packaging/aur/`](packaging/aur). It builds from the tagged source tarball:

```sh
cd packaging/aur && makepkg -si
```

### From source

```sh
git clone git@github.com:anisharaz/niri-sidestage.git
cd niri-sidestage
cargo build --release
install -Dm755 target/release/niri-sidestage ~/.local/bin/niri-sidestage
```

## Start with niri

Add this to your niri config. Use the full path if `~/.local/bin` is not on niri's `PATH`:

```kdl
spawn-at-startup "niri-sidestage"
```

To open it from the keyboard, bind the control command:

```kdl
binds {
    Mod+Tab { spawn "niri-sidestage" "toggle"; }
}
```

## Usage

```
niri-sidestage                 run the strip (auto-hides, slides out when the pointer touches the edge)
niri-sidestage --pinned        run the strip and keep it open
niri-sidestage toggle          open the panel on the focused monitor, or dismiss it
niri-sidestage open | close    the same, one direction only
niri-sidestage toggle --output eDP-1    target a specific monitor
niri-sidestage --version
```

Move the pointer to the edge of a monitor, level with the panel. The thin blue marker in the
middle of the edge shows the trigger zone: it is as tall as the panel (at least 200px), so the rest
of the edge, including the corners, does nothing.

A panel opened with `toggle` or `open` stays open and holds the keyboard until you pick something,
press Escape, or toggle it again. Only one copy can run at a time; starting a second prints a
message and exits.

## Configuration

An optional file at `~/.config/niri-sidestage/config.toml` (or under `$XDG_CONFIG_HOME`). It is a
flat list of `key = value` lines; `#` starts a comment. Everything is optional:

```toml
side = "left"          # "left" or "right"
gap = "auto"           # px width of the edge trigger; "auto" reads `gaps` from your niri config
panel_width = 152      # 100 to 600
hide_delay_ms = 250    # how long after the pointer leaves before the panel slides away
slide_ms = 170         # slide animation time
min_trigger = 200      # the trigger zone is as tall as the panel, but at least this tall
```

The gap should equal niri's `gaps` so the trigger stays inside the gap and never covers a window.
With `gap = "auto"` (the default) it follows your niri config, including `include`d files, and falls
back to 5 if it finds nothing. Restart niri-sidestage after editing the file.

Environment variables:

| Variable | Meaning |
|---|---|
| `NIRI_SIDESTAGE_GAP` | Edge trigger width in pixels. Overrides the config file and auto-detection. |
| `NIRI_SIDESTAGE_DEBUG` | Print pointer enter/leave/motion events to stderr. |
| `NIRI_SIDESTAGE_START_WS` | Start in the workspace breakdown view. |

## How it works

One process, no daemon.

- It follows niri's event stream (`niri-ipc`) and keeps a snapshot of workspaces and windows per
  output. Clicks send `FocusWindow`, `FocusWorkspace`, `CloseWindow` or `MoveWindowToWorkspace`
  actions back over the IPC socket.
- Per monitor there is one full-height layer-shell window anchored to the chosen edge. An input
  region keeps only the trigger zone interactive while closed, and the zone plus the visible panel
  while open. Because it is a single surface, the pointer never hands off between windows, which
  avoids flicker from enter/leave races.
- The panel's window has a fixed size. Opening and closing slides the content inside it, driven by
  the frame clock. Resizing a layer-shell window on every frame lags behind the compositor and tears.
- `toggle`, `open` and `close` talk to the running strip over a Unix socket at
  `$XDG_RUNTIME_DIR/niri-sidestage.sock`. While opened that way the panel takes exclusive keyboard
  focus, and releases it when dismissed.

## Limitations

- The open panel covers roughly the leftmost (or rightmost) 160px of the window beneath it. It
  reserves no space on purpose, so it cannot avoid that.
- No window thumbnails, only app icons and titles.
- You can only drop a window onto a workspace that is shown in the breakdown, so not onto an empty
  one.
- Tested on niri 26.04 on Arch Linux with two 1080p monitors at scale 1. Other setups, such as
  fractional scaling, are untested, and so is the bottom edge of the trigger zone with
  `side = "right"`.
- niri's default top-left hot corner opens the overview. If you reach the edge by sliding up along
  it, you may brush that corner. You can disable it with `hot-corners { off }` in your niri config.

## License

[MIT](LICENSE)
