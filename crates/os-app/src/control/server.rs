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
    Ok(Server { rx, level, clients: Default::default(), at: shown })
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
            let mut client = server.clients.remove(&req.conn).unwrap_or_else(|| Client { name: format!("Client {}", req.conn), level });
            if let Some(a) = self.rpc_line(&mut client, &req.line) {
                let _ = req.reply.send(a);
            }
            server.clients.insert(req.conn, client);
        }
        self.rpc_server = Some(server);
    }
}
