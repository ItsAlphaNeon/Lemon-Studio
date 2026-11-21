// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The interface's side of `crate::control::server` (step 9: the session's side stays there).

#![allow(unused_imports)]
use std::io::{BufRead, BufReader, Write};
use std::sync::mpsc::{channel, Receiver, Sender};
use super::rpc::Client;
use super::Level;
#[allow(unused_imports)]
use crate::control::server::*;

impl crate::app::App {
    /// Answers `server`'s clients from now on.
    pub fn listen(&mut self, server: Server) {
        self.state.session.rpc_server = Some(server);
        self.state.session.control.watched = true;
    }

    /// Answers the requests waiting (on every tick).
    pub fn control_tick(&mut self) {
        let Some(mut server) = self.state.session.rpc_server.take() else { return };
        while let Ok(req) = server.rx.try_recv() {
            let level = server.level;
            let headless = server.headless;
            let token = server.token.clone();
            let mut client =
                server.clients.remove(&req.conn).unwrap_or_else(|| Client { name: format!("Client {}", req.conn), level, saves: headless, token });
            if let Some(a) = self.rpc_line(&mut client, &req.line) {
                let _ = req.reply.send(a);
            }
            server.clients.insert(req.conn, client);
        }
        self.state.session.rpc_server = Some(server);
    }
}
