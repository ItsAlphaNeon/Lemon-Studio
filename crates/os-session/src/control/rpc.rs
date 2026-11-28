// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The JSON-RPC 2.0 methods clients call (over `server`'s socket, one request per line). Nothing here is about any
//! one client: an AI assistant, a script and a test speak the same methods.
//!
//! | Method | Params | Level | Result |
//! |---|---|---|---|
//! | `hello` | `name`, `token` (on a port) | read | the app, the protocol version, the client's level |
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
//! | `save` | `path` (else the project's file) | edit | where it was saved; with a window the user is asked instead |
//!
//! A client that proposes waits for the user: the app asks them (`App::ask_proposal`).

use serde_json::{json, Value};

use super::{By, Command, Level};
use crate::session::Session;

/// The protocol's version (a client checks it in `hello`'s result).
pub const PROTOCOL: u32 = 1;

/// A connected client: the name it gave and what it may do.
#[derive(Debug, Clone)]
pub struct Client {
    pub name: String,
    pub level: Level,
    /// It saves the project itself (`save`): only without a window (`--serve`), where no one could be asked.
    pub saves: bool,
    /// The token it must give in `hello` before anything else (a port's, `server::Server::token`); `None` once given.
    pub token: Option<String>,
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

/// What a request leaves to an interface (`rpc_in`): a proposal to show, a save to ask the user about.
#[derive(Debug, Default)]
pub struct Asks {
    pub proposal: Option<u64>,
    pub save: Option<(String, std::path::PathBuf)>,
}

/// Answers one request line from client `c` (`None`: a notification, no answer).
pub fn rpc_line_in(ses: &mut Session, c: &mut Client, line: &str, selected: usize, asks: &mut Asks) -> Option<String> {
    let (id, out) = match serde_json::from_str::<Value>(line) {
        Err(e) => (Value::Null, Err(Fail(PARSE, e.to_string()))),
        Ok(req) => {
            let id = req.get("id").cloned();
            let method = req.get("method").and_then(Value::as_str);
            let params = req.get("params").cloned().unwrap_or(Value::Null);
            let out = match method {
                Some(m) if req.get("jsonrpc").and_then(Value::as_str) == Some("2.0") => rpc_in(ses, c, m, &params, selected, asks),
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
pub fn rpc_in(ses: &mut Session, c: &mut Client, m: &str, params: &Value, selected: usize, asks: &mut Asks) -> Result<Value, Fail> {
    if let Some(t) = &c.token {
        if m != "hello" || params.get("token").and_then(Value::as_str) != Some(t.as_str()) {
            return Err(Fail(NOT_ALLOWED, "give the token in `hello` first".into()));
        }
        c.token = None;
    }
    let by = By::Client(c.name.clone());
    let out = match m {
        "hello" => {
            if let Some(n) = params.get("name").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty()) {
                c.name = n.chars().take(40).collect();
            }
            Ok(json!({ "app": "Lemon Studio", "version": crate::version::VERSION, "protocol": PROTOCOL, "name": c.name, "level": c.level }))
        }
        "describe" => Ok(super::describe()),
        "state" => Ok(super::query::project_in(ses, selected)),
        "notes" => super::query::notes(ses, uint(params, "pattern")? as u32).map_err(bad),
        "plugins" => {
            let names = |v: &[crate::plugins::Available]| v.iter().map(|a| a.desc.name.clone()).collect::<Vec<_>>();
            Ok(json!({ "instruments": names(&ses.plugins.available), "effects": names(&ses.plugins.effects) }))
        }
        "params" => params_of(ses, uint(params, "owner")? as u32),
        "history" => {
            let h = &ses.history;
            let steps: Vec<Value> = h.entries(&ses.settings).into_iter().map(|(k, name)| json!({ "step": k, "name": name, "undone": h.undone(k) })).collect();
            Ok(json!({ "at": h.at(), "steps": steps }))
        }
        "events" => {
            let since = params.get("since").and_then(Value::as_u64).unwrap_or(0);
            let (events, last) = ses.control.events_since(since);
            Ok(json!({ "events": events, "last": last }))
        }
        "proposals" => {
            let list: Vec<Value> = ses.control.proposals.iter().map(|p| json!({ "proposal": p.n, "name": p.name, "changes": p.changes })).collect();
            Ok(Value::Array(list))
        }
        "apply" => {
            need(c, Level::Edit)?;
            let mut cmds = commands(params)?;
            fill_params(ses, &mut cmds).map_err(failed)?;
            let results = match params.get("gesture").and_then(Value::as_u64) {
                Some(g) => super::apply_run_in(ses, &by, g, cmds),
                None => super::apply_in(ses, &by, params.get("name").and_then(Value::as_str), cmds),
            }
            .map_err(failed)?;
            Ok(json!({ "results": results }))
        }
        "begin" => {
            need(c, Level::Edit)?;
            Ok(json!({ "gesture": super::begin_in(ses, params.get("name").and_then(Value::as_str)) }))
        }
        "end" => {
            need(c, Level::Edit)?;
            super::end_in(ses, uint(params, "gesture")?).map_err(failed)?;
            Ok(Value::Null)
        }
        "propose" => {
            need(c, Level::Propose)?;
            let mut cmds = commands(params)?;
            fill_params(ses, &mut cmds).map_err(failed)?;
            let name = params.get("name").and_then(Value::as_str).unwrap_or("Proposal");
            let (n, changes) = super::propose_in(ses, &by, name, cmds, selected).map_err(failed)?;
            asks.proposal = Some(n);
            Ok(json!({ "proposal": n, "changes": changes }))
        }
        "accept" => {
            need(c, Level::Edit)?;
            let results = super::accept_in(ses, uint(params, "proposal")?).map_err(failed)?;
            Ok(json!({ "results": results }))
        }
        "discard" => {
            need(c, Level::Edit)?;
            super::discard(ses, uint(params, "proposal")?).map_err(failed)?;
            Ok(Value::Null)
        }
        "undo" | "redo" => {
            need(c, Level::Edit)?;
            let name = if m == "undo" { crate::history::undo_in(ses) } else { crate::history::redo_in(ses) };
            if let Some(n) = &name {
                let tempo = ses.tempo();
                ses.control.saw(&ses.project, tempo);
                ses.control.event(&by, if m == "undo" { "undo" } else { "redo" }, n.clone(), Vec::new());
            }
            Ok(json!({ "step": name }))
        }
        // Where a controller's notes play in a session without a window (a window's selected channel decides there).
        "midi_target" => {
            need(c, Level::Edit)?;
            let id = match params.get("channel") {
                None | Some(Value::Null) => None,
                Some(v) => {
                    let id = v.as_u64().and_then(|v| u32::try_from(v).ok()).ok_or_else(|| bad("`channel`: a channel's id"))?;
                    ses.project.channels.iter().any(|ch| ch.id.0 == id).then_some(id).ok_or_else(|| bad(format!("no channel {id}")))?;
                    Some(id)
                }
            };
            ses.midi_channel = id;
            Ok(Value::Null)
        }
        // Writing a file is an effect outside the project: with a window, the user decides (spec:
        // roadmap/control-plane.md "Safety model").
        "save" => {
            need(c, Level::Edit)?;
            let path = params.get("path").and_then(Value::as_str).map(std::path::PathBuf::from).or_else(|| ses.path.clone());
            let path = path.ok_or_else(|| bad("the project has no file yet: give `path`"))?;
            if c.saves {
                let (result, msg) = crate::project_io::save(ses, &path);
                ses.touched.views.push(crate::session::View::Message(msg.clone()));
                if result.is_err() {
                    return Err(failed(msg));
                }
                Ok(json!({ "saved": path.display().to_string() }))
            } else {
                asks.save = Some((c.name.clone(), path.clone()));
                Ok(json!({ "asked": path.display().to_string() }))
            }
        }
        _ => Err(Fail(NO_METHOD, format!("no method `{m}`"))),
    };
    if matches!(m, "apply" | "accept" | "undo" | "redo") && out.is_ok() {
        ses.touched.repaint = true;
    }
    out
}

/// The parameters of the plugin playing channel or slot `owner`.
fn params_of(ses: &mut Session, owner: u32) -> Result<Value, Fail> {
    let key = super::command::owner_key(&*ses, owner).ok_or_else(|| bad(format!("no channel or slot plugin {owner}")))?;
    let h = ses.plugins.hosted.get(&key).ok_or_else(|| failed(format!("{owner} is not played by a hosted plugin")))?;
    let p = h.plugin.lock().map_err(|_| failed("the plugin is busy"))?;
    let mut list = Vec::new();
    for i in 0..p.param_count() {
        let Some(info) = p.param_info(i) else { continue };
        if info.hidden {
            continue;
        }
        let v = p.param_value(info.id);
        if let Some(v) = v {
            ses.control.saw_param(owner, info.id, v);
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
pub fn fill_params(ses: &mut Session, cmds: &mut [Command]) -> Result<(), String> {
    for c in cmds.iter_mut() {
        if let Command::SetParam { owner, param, from: from @ None, .. } = c {
            let key = super::command::owner_key(&*ses, *owner).ok_or_else(|| format!("no channel or slot plugin {owner}"))?;
            let v = ses
                .plugins
                .hosted
                .get(&key)
                .and_then(|h| h.plugin.lock().ok()?.param_value(*param))
                .ok_or_else(|| format!("{owner} has no parameter {param}"))?;
            *from = Some(v);
            ses.control.saw_param(*owner, *param, v);
        }
    }
    Ok(())
}
