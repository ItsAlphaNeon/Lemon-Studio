// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The local socket clients connect to (`--control <where>`): a Unix socket at a path (Linux, macOS), or a TCP port on
//! 127.0.0.1 (Windows, or a number given). One JSON-RPC request per line, one answer per line. Each connection has a
//! thread that reads it; requests go to the UI thread, which answers them on its tick (`App::control_tick`), so they
//! see and change the project as the interface does. Only this machine can connect.

use std::io::{BufRead, BufReader, Write};
use std::sync::mpsc::{channel, Receiver, Sender};

use super::rpc::Client;
use super::Level;

/// A request line from connection `conn`, and where its answer goes.
pub struct Request {
    pub conn: u64,
    pub line: String,
    pub reply: Sender<String>,
}

/// The UI thread's end: requests waiting, and the clients connected.
pub struct Server {
    pub rx: Receiver<Request>,
    pub level: Level,
    /// Serving without a window (`--serve`): there is no one to ask, so clients save the project themselves.
    pub headless: bool,
    pub clients: std::collections::HashMap<u64, Client>,
    /// Where it listens, as the hint bar names it.
    pub at: String,
}

/// Starts listening at `at` (a socket path, or a port number); `wake` wakes the UI thread when a request comes.
pub fn start(at: &str, level: Level, wake: fn()) -> std::io::Result<Server> {
    let (tx, rx) = channel::<Request>();
    let shown;
    #[cfg(unix)]
    {
        if at.parse::<u16>().is_ok() {
            shown = tcp(at, tx, wake)?;
        } else {
            // A socket left by an earlier run that ended without removing it.
            let _ = std::fs::remove_file(at);
            let l = std::os::unix::net::UnixListener::bind(at)?;
            shown = at.to_string();
            std::thread::Builder::new().name("control".into()).spawn(move || {
                for (conn, s) in l.incoming().flatten().enumerate() {
                    if let Ok(w) = s.try_clone() {
                        serve(conn as u64 + 1, BufReader::new(s), w, tx.clone(), wake);
                    }
                }
            })?;
        }
    }
    #[cfg(not(unix))]
    {
        shown = tcp(at, tx, wake)?;
    }
    Ok(Server { rx, level, headless: false, clients: Default::default(), at: shown })
}

fn tcp(at: &str, tx: Sender<Request>, wake: fn()) -> std::io::Result<String> {
    let port: u16 = at.parse().map_err(|_| std::io::Error::other(format!("not a port: {at}")))?;
    let l = std::net::TcpListener::bind(("127.0.0.1", port))?;
    let shown = l.local_addr()?.to_string();
    std::thread::Builder::new().name("control".into()).spawn(move || {
        for (conn, s) in l.incoming().flatten().enumerate() {
            if let Ok(w) = s.try_clone() {
                serve(conn as u64 + 1, BufReader::new(s), w, tx.clone(), wake);
            }
        }
    })?;
    Ok(shown)
}

/// One connection's thread: each line to the UI thread, each answer back.
fn serve<R: BufRead + Send + 'static, W: Write + Send + 'static>(conn: u64, r: R, mut w: W, tx: Sender<Request>, wake: fn()) {
    let _ = std::thread::Builder::new().name(format!("control {conn}")).spawn(move || {
        for line in r.lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            let (reply, answer) = channel();
            if tx.send(Request { conn, line, reply }).is_err() {
                break;
            }
            wake();
            // A notification gets no answer: the UI thread drops `reply`.
            if let Ok(a) = answer.recv() {
                if writeln!(w, "{a}").and_then(|_| w.flush()).is_err() {
                    break;
                }
            }
        }
    });
}

impl crate::app::App {
    /// Answers the requests waiting (on every tick).
    pub fn control_tick(&mut self) {
        let Some(mut server) = self.rpc_server.take() else { return };
        while let Ok(req) = server.rx.try_recv() {
            let level = server.level;
            let headless = server.headless;
            let mut client = server.clients.remove(&req.conn).unwrap_or_else(|| Client { name: format!("Client {}", req.conn), level, saves: headless });
            if let Some(a) = self.rpc_line(&mut client, &req.line) {
                let _ = req.reply.send(a);
            }
            server.clients.insert(req.conn, client);
        }
        self.rpc_server = Some(server);
    }
}

/// The control plane's command-line flags: `--control <where>` (listen beside the window), `--serve <where>` (listen
/// without a window), `--control-level read|propose|edit` (what clients may do; edit by default).
#[derive(Debug, Default, PartialEq)]
pub struct Flags {
    pub control: Option<String>,
    pub serve: Option<String>,
    pub level: Option<Level>,
}

impl Flags {
    /// Takes the flags out of `args` (the rest stay: the project to open).
    pub fn take(args: &mut Vec<String>) -> Result<Flags, String> {
        let mut f = Flags::default();
        let mut i = 0;
        while i < args.len() {
            let slot = match args[i].as_str() {
                "--control" => &mut f.control,
                "--serve" => &mut f.serve,
                "--control-level" => {
                    let v = args.get(i + 1).ok_or("--control-level needs read, propose or edit")?;
                    f.level = Some(Level::parse(v).ok_or_else(|| format!("--control-level: {v} is not read, propose or edit"))?);
                    args.drain(i..i + 2);
                    continue;
                }
                _ => {
                    i += 1;
                    continue;
                }
            };
            let v = args.get(i + 1).filter(|v| !v.starts_with("--")).ok_or_else(|| format!("{} needs a socket path or a port", args[i]))?;
            *slot = Some(v.clone());
            args.drain(i..i + 2);
        }
        if f.control.is_some() && f.serve.is_some() {
            return Err("--control and --serve: one or the other".into());
        }
        Ok(f)
    }
}

/// `--serve`: the app without a window, `project` opened, answering clients until the process is ended.
pub fn serve_headless(at: &str, level: Level, project: Option<std::path::PathBuf>) -> ! {
    let mut app = crate::app::App::new(project);
    // The plugins of the last scan, as the window's app has them before its own scan.
    app.load_plugins();
    let mut server = match start(at, level, || {}) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Lemon Studio: could not listen at {at}: {e}");
            std::process::exit(1);
        }
    };
    server.headless = true;
    eprintln!("Lemon Studio: serving the control plane at {} ({:?})", server.at, level);
    app.rpc_server = Some(server);
    loop {
        app.tick();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
