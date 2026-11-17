//! niri-sidestage: a side strip window and workspace switcher for niri.
//!
//! Windows stay exactly where niri put them. This only draws a strip of the windows on the
//! current workspace at the edge of each monitor; clicking a tile focuses that window and
//! niri scrolls it into view.

mod config;
mod control;
mod niri;
mod strip;

const USAGE: &str = "\
niri-sidestage: side strip window and workspace switcher for niri

usage:
  niri-sidestage                  run the strip (auto-hides, slides out when the pointer
                                  touches the screen edge next to it)
  niri-sidestage --pinned         run the strip and keep it open
  niri-sidestage toggle|open|close [--output NAME]
                                  control the running strip (default: the focused monitor)
  niri-sidestage --version

config file: ~/.config/niri-sidestage/config.toml
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => start(false),
        Some("--pinned") => start(true),
        Some("--version" | "-V") => println!("niri-sidestage {}", env!("CARGO_PKG_VERSION")),
        Some(a) if control::Act::parse(a).is_some() => command(control::Act::parse(a).unwrap(), &args[1..]),
        _ => print!("{USAGE}"),
    }
}

fn start(pinned: bool) {
    if control::running() {
        eprintln!("niri-sidestage is already running");
        std::process::exit(1);
    }
    strip::run(pinned);
}

fn command(act: control::Act, rest: &[String]) {
    let output = match rest {
        [] => None,
        [flag, name] if flag == "--output" => Some(name.clone()),
        _ => {
            print!("{USAGE}");
            std::process::exit(2);
        }
    };
    if let Err(e) = control::send(act, output) {
        eprintln!("niri-sidestage: {e}");
        std::process::exit(1);
    }
}
