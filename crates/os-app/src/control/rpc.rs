// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The JSON-RPC 2.0 methods clients call (over `server`'s socket, one request per line). Nothing here is about any
//! one client: an AI assistant, a script and a test speak the same methods.
//!
//! | Method | Params | Level | Result |
//! |---|---|---|---|
//! | `hello` | `name` | read | the app, the protocol version, the client's level |
//! | `state` | | read | the project (`query::project`) |
//! | `notes` | `pattern` | read | a pattern's notes |
//! | `plugins` | | read | the instruments and effects commands can add |
//! | `params` | `owner` | read | the parameters of the plugin playing a channel or slot |
//! | `history` | | read | the undo history and where the project is in it |
//! | `events` | `since` | read | what changed since event `since`, and the newest event's number |
//! | `apply` | `commands` (or `command`), `name` | edit | each command's result; one undo step |
//! | `propose` | `commands`, `name` | propose | the proposal's number and what it would change |
//! | `proposals` | | read | the proposals waiting |
//! | `accept`, `discard` | `proposal` | edit | the commands' results; nothing |
//! | `undo`, `redo` | | edit | the step undone or redone |
//!
//! A client that proposes waits for the user: the app asks them (`App::ask_proposal`).

use serde_json::{json, Value};

use super::{By, Command, Level};
use crate::app::App;

/// The protocol's version (a client checks it in `hello`'s result).
pub const PROTOCOL: u32 = 1;

/// A connected client: the name it gave and what it may do.
#[derive(Debug, Clone)]
pub struct Client {
    pub name: String,
    pub level: Level,
}

/// A JSON-RPC error: its code and message.
pub struct Fail(i64, String);

const PARSE: i64 = -32700;
const INVALID: i64 = -32600;
const NO_METHOD: i64 = -32601;
const BAD_PARAMS: i64 = -32602;
const FAILED: i64 = -32000;
const NOT_ALLOWED: i64 = -32001;

fn bad(e: impl std::fmt::Display) -> Fail {
    Fail(BAD_PARAMS, e.to_string())
}

fn failed(e: impl std::fmt::Display) -> Fail {
    Fail(FAILED, e.to_string())
}

fn need(c: &Client, level: Level) -> Result<(), Fail> {
    if c.level >= level {
        Ok(())
    } else {
        Err(Fail(NOT_ALLOWED, format!("{} may only {:?}", c.name, c.level).to_lowercase()))
    }
}

fn uint(params: &Value, key: &str) -> Result<u64, Fail> {
    params.get(key).and_then(Value::as_u64).ok_or_else(|| bad(format!("`{key}` is missing or not a number")))
}

fn commands(params: &Value) -> Result<Vec<Command>, Fail> {
    let list = match (params.get("commands"), params.get("command")) {
        (Some(v), None) => v.clone(),
        (None, Some(v)) => Value::Array(vec![v.clone()]),
        _ => return Err(bad("give `commands` (a list) or `command`")),
    };
    serde_json::from_value(list).map_err(bad)
}

impl App {
    /// Answers one request line from client `c` (`None`: a notification, no answer).
    pub fn rpc_line(&mut self, c: &mut Client, line: &str) -> Option<String> {
        let (id, out) = match serde_json::from_str::<Value>(line) {
            Err(e) => (Value::Null, Err(Fail(PARSE, e.to_string()))),
            Ok(req) => {
                let id = req.get("id").cloned();
                let method = req.get("method").and_then(Value::as_str);
                let params = req.get("params").cloned().unwrap_or(Value::Null);
                let out = match method {
                    Some(m) if req.get("jsonrpc").and_then(Value::as_str) == Some("2.0") => self.rpc(c, m, &params),
                    _ => Err(Fail(INVALID, "not a JSON-RPC 2.0 request".into())),
                };
                match id {
                    None => return None,
                    Some(id) => (id, out),
                }
            }
        };
        let reply = match out {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(Fail(code, message)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
        };
        Some(reply.to_string())
    }

    /// Method `m` with `params` for client `c`.
    pub fn rpc(&mut self, c: &mut Client, m: &str, params: &Value) -> Result<Value, Fail> {
        let by = By::Client(c.name.clone());
        let out = match m {
            "hello" => {
                if let Some(n) = params.get("name").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty()) {
                    c.name = n.chars().take(40).collect();
                }
                Ok(json!({ "app": "Lemon Studio", "version": env!("CARGO_PKG_VERSION"), "protocol": PROTOCOL, "name": c.name, "level": c.level }))
            }
            "state" => Ok(super::query::project(&self.state)),
            "notes" => super::query::notes(&self.state, uint(params, "pattern")? as u32).map_err(bad),
            "plugins" => {
                let names = |v: &[crate::plugins::Available]| v.iter().map(|a| a.desc.name.clone()).collect::<Vec<_>>();
                Ok(json!({ "instruments": names(&self.plugins.available), "effects": names(&self.plugins.effects) }))
            }
            "params" => self.params_of(uint(params, "owner")? as u32),
            "history" => {
                let h = &self.state.history;
                let steps: Vec<Value> = h.entries().into_iter().map(|(k, name)| json!({ "step": k, "name": name, "undone": h.undone(k) })).collect();
                Ok(json!({ "at": h.at(), "steps": steps }))
            }
            "events" => {
                let since = params.get("since").and_then(Value::as_u64).unwrap_or(0);
                let (events, last) = self.state.control.events_since(since);
                Ok(json!({ "events": events, "last": last }))
            }
            "proposals" => {
                let list: Vec<Value> = self.state.control.proposals.iter().map(|p| json!({ "proposal": p.n, "name": p.name, "changes": p.changes })).collect();
                Ok(Value::Array(list))
            }
            "apply" => {
                need(c, Level::Edit)?;
                let mut cmds = commands(params)?;
                self.fill_params(&mut cmds).map_err(failed)?;
                let name = params.get("name").and_then(Value::as_str);
                let results = super::apply(&mut self.state, &by, name, cmds).map_err(failed)?;
                Ok(json!({ "results": results }))
            }
            "propose" => {
                need(c, Level::Propose)?;
                let mut cmds = commands(params)?;
                self.fill_params(&mut cmds).map_err(failed)?;
                let name = params.get("name").and_then(Value::as_str).unwrap_or("Proposal");
                let (n, changes) = super::propose(&mut self.state, &by, name, cmds).map_err(failed)?;
                self.ask_proposal(n);
                Ok(json!({ "proposal": n, "changes": changes }))
            }
            "accept" => {
                need(c, Level::Edit)?;
                let results = super::accept(&mut self.state, uint(params, "proposal")?).map_err(failed)?;
                Ok(json!({ "results": results }))
            }
            "discard" => {
                need(c, Level::Edit)?;
                super::discard(&mut self.state, uint(params, "proposal")?).map_err(failed)?;
                Ok(Value::Null)
            }
            "undo" | "redo" => {
                need(c, Level::Edit)?;
                let s = &mut self.state;
                let name = if m == "undo" { crate::history::undo(s) } else { crate::history::redo(s) };
                if let Some(n) = &name {
                    s.control.saw(&s.project, s.tempo);
                    s.control.event(&by, if m == "undo" { "undo" } else { "redo" }, n.clone(), Vec::new());
                }
                Ok(json!({ "step": name }))
            }
            _ => Err(Fail(NO_METHOD, format!("no method `{m}`"))),
        };
        if matches!(m, "apply" | "accept" | "undo" | "redo") && out.is_ok() {
            self.invalidate();
        }
        out
    }

    /// The parameters of the plugin playing channel or slot `owner`.
    fn params_of(&mut self, owner: u32) -> Result<Value, Fail> {
        let key = super::command::owner_key(&self.state.project, owner).ok_or_else(|| bad(format!("no channel or slot plugin {owner}")))?;
        let h = self.plugins.hosted.get(&key).ok_or_else(|| failed(format!("{owner} is not played by a hosted plugin")))?;
        let p = h.plugin.lock().map_err(|_| failed("the plugin is busy"))?;
        let mut list = Vec::new();
        for i in 0..p.param_count() {
            let Some(info) = p.param_info(i) else { continue };
            if info.hidden {
                continue;
            }
            let v = p.param_value(info.id);
            if let Some(v) = v {
                self.state.control.saw_param(owner, info.id, v);
            }
            list.push(json!({
                "id": info.id,
                "name": info.name,
                "group": info.group,
                "unit": info.unit,
                "value": v,
                "text": v.and_then(|v| p.param_text(info.id, v)),
                "min": info.min,
                "max": info.max,
                "default": info.default,
                "steps": info.steps,
                "list": info.enumerated,
                "automatable": info.automatable,
                "read_only": info.read_only,
            }));
        }
        Ok(json!({ "owner": owner, "plugin": h.name, "params": list }))
    }

    /// Gives each `SetParam` its value before (`from`), read from the plugin.
    pub(crate) fn fill_params(&mut self, cmds: &mut [Command]) -> Result<(), String> {
        for c in cmds.iter_mut() {
            if let Command::SetParam { owner, param, from: from @ None, .. } = c {
                let key = super::command::owner_key(&self.state.project, *owner).ok_or_else(|| format!("no channel or slot plugin {owner}"))?;
                let v = self
                    .plugins
                    .hosted
                    .get(&key)
                    .and_then(|h| h.plugin.lock().ok()?.param_value(*param))
                    .ok_or_else(|| format!("{owner} has no parameter {param}"))?;
                *from = Some(v);
                self.state.control.saw_param(*owner, *param, v);
            }
        }
        Ok(())
    }
}

impl App {
    /// Asks the user about proposal `n`: what it would change, Apply or Discard (spec: roadmap/control-plane.md
    /// "Proposals").
    pub(crate) fn ask_proposal(&mut self, n: u64) {
        use crate::dialog::{Answer, Kind, Msg, Then};
        let Some(p) = self.state.control.proposals.iter().find(|p| p.n == n) else { return };
        let who = match &p.by {
            super::By::Client(c) => c.clone(),
            super::By::User => "You".into(),
        };
        const SHOWN: usize = 8;
        let mut lines: Vec<String> = p.changes.iter().take(SHOWN).map(|c| format!("\n  {}: {} > {}", c.path, c.before, c.after)).collect();
        if p.changes.len() > SHOWN {
            lines.push(format!("\n  and {} more", p.changes.len() - SHOWN));
        }
        let head = format!(" proposes \"{}\" ({} change{}):", p.name, p.changes.len(), if p.changes.len() == 1 { "" } else { "s" });
        let text = vec![(who, true), (head, false), (lines.concat(), false)];
        let mut msg = Msg::new(Kind::Info, "Proposal", text, &[Answer::Yes, Answer::No], Then::Proposal(n));
        msg.labels = vec!["Apply", "Discard"];
        self.show_msg(msg);
    }
}
