//! A tiny control socket so a keybinding can open the strip: `niri-sidestage toggle`.
//!
//! The running strip listens on `$XDG_RUNTIME_DIR/niri-sidestage.sock`. A command is one line,
//! `<toggle|open|close> <output>`, answered with `ok` or an error line.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::thread;

#[derive(Clone, Copy, Debug)]
pub enum Act {
    Toggle,
    Open,
    Close,
}

impl Act {
    pub fn parse(s: &str) -> Option<Act> {
        match s {
            "toggle" => Some(Act::Toggle),
            "open" => Some(Act::Open),
            "close" => Some(Act::Close),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Act::Toggle => "toggle",
            Act::Open => "open",
            Act::Close => "close",
        }
    }
}

#[derive(Debug)]
pub struct Cmd {
    pub act: Act,
    pub output: String,
}

fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    dir.join("niri-sidestage.sock")
}

/// Is a strip already listening?
pub fn running() -> bool {
    UnixStream::connect(socket_path()).is_ok()
}

/// Start listening; commands are forwarded to `tx`. Fails if another instance is running.
pub fn serve(tx: Sender<Cmd>) -> std::io::Result<()> {
    let path = socket_path();
    if running() {
        return Err(std::io::Error::new(std::io::ErrorKind::AddrInUse, "already running"));
    }
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path)?;
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let tx = tx.clone();
            thread::spawn(move || handle(stream, tx));
        }
    });
    Ok(())
}

fn handle(mut stream: UnixStream, tx: Sender<Cmd>) {
    let mut line = String::new();
    if BufReader::new(&stream).read_line(&mut line).is_err() {
        return;
    }
    let mut parts = line.split_whitespace();
    let reply = match (parts.next().and_then(Act::parse), parts.next()) {
        (Some(act), Some(output)) => {
            let _ = tx.send(Cmd { act, output: output.to_string() });
            "ok\n"
        }
        _ => "error: expected `<toggle|open|close> <output>`\n",
    };
    let _ = stream.write_all(reply.as_bytes());
}

/// Send a command to the running strip. Without an output, the focused monitor is used.
pub fn send(act: Act, output: Option<String>) -> Result<(), String> {
    let output = match output.or_else(crate::niri::focused_output) {
        Some(o) => o,
        None => return Err("could not tell which monitor is focused; pass --output NAME".into()),
    };
    let mut stream = UnixStream::connect(socket_path())
        .map_err(|_| "niri-sidestage is not running".to_string())?;
    writeln!(stream, "{} {}", act.name(), output).map_err(|e| e.to_string())?;
    let mut reply = String::new();
    BufReader::new(&stream).read_line(&mut reply).map_err(|e| e.to_string())?;
    if reply.trim() == "ok" { Ok(()) } else { Err(reply.trim().to_string()) }
}
