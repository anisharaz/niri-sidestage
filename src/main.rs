//! niri-sidestage: a side strip window and workspace switcher for niri.
//!
//! Windows stay exactly where niri put them. This only draws a strip of the windows on the
//! current workspace on the left edge of each monitor; clicking a tile focuses that window and
//! niri scrolls it into view.

mod niri;
mod strip;

const USAGE: &str = "\
niri-sidestage: side strip window and workspace switcher for niri

usage:
  niri-sidestage            run the strip (auto-hides, slides out when the pointer hits the left edge)
  niri-sidestage --pinned   keep the strip open
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => strip::run(false),
        Some("--pinned") => strip::run(true),
        _ => print!("{USAGE}"),
    }
}
