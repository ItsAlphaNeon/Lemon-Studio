// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! A client of Lemon Studio's control socket (docs/control-plane.md), using nothing of the program but its protocol:
//! JSON-RPC 2.0, one request and one answer per line, over a Unix socket path or a local port. The monitor's window is
//! one user of it; a terminal view or another program's interface can be others.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};

trait Stream: std::io::Read + Write + Send {}
impl<T: std::io::Read + Write + Send> Stream for T {}

/// A connection to a session, said hello to.
pub struct Client {
    read: BufReader<Box<dyn Stream>>,
    write: Box<dyn Stream>,
    next: u64,
    /// What the session answered to `hello`: its version, protocol and the level this client has.
    pub hello: Value,
}

/// Why a call gave no result: the connection's error, or the session's own (its code and message).
#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Session(i64, String),
    Bad(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Session(_, m) => f.write_str(m),
            Error::Bad(m) => f.write_str(m),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Error {
        Error::Io(e)
    }
}

impl Client {
    /// Connects to `at` (a socket path, or a port number on this machine) and says hello as `name`, giving `token`
    /// when the session asks for one.
    pub fn connect(at: &str, name: &str, token: Option<&str>) -> Result<Client, Error> {
        let (read, write): (Box<dyn Stream>, Box<dyn Stream>) = if let Ok(port) = at.parse::<u16>() {
            let s = std::net::TcpStream::connect(("127.0.0.1", port))?;
            (Box::new(s.try_clone()?), Box::new(s))
        } else {
            #[cfg(unix)]
            {
                let s = std::os::unix::net::UnixStream::connect(at)?;
                (Box::new(s.try_clone()?), Box::new(s))
            }
            #[cfg(not(unix))]
            return Err(Error::Bad(format!("not a port: {at}")));
        };
        let mut c = Client { read: BufReader::new(read), write, next: 0, hello: Value::Null };
        let mut params = json!({ "name": name });
        if let Some(t) = token {
            params["token"] = json!(t);
        }
        c.hello = c.call("hello", params)?;
        Ok(c)
    }

    /// Calls `method` with `params` and waits for its answer.
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, Error> {
        self.next += 1;
        let line = json!({ "jsonrpc": "2.0", "id": self.next, "method": method, "params": params }).to_string();
        self.write.write_all(line.as_bytes())?;
        self.write.write_all(b"\n")?;
        self.write.flush()?;
        let mut answer = String::new();
        if self.read.read_line(&mut answer)? == 0 {
            return Err(Error::Bad("the session closed the connection".into()));
        }
        let v: Value = serde_json::from_str(&answer).map_err(|e| Error::Bad(format!("not an answer: {e}")))?;
        if let Some(e) = v.get("error") {
            return Err(Error::Session(e["code"].as_i64().unwrap_or(0), e["message"].as_str().unwrap_or("").to_string()));
        }
        Ok(v.get("result").cloned().unwrap_or(Value::Null))
    }

    /// The project as the session holds it (`state`).
    pub fn state(&mut self) -> Result<Value, Error> {
        self.call("state", json!({}))
    }

    /// The events after `since`, and the number of the last one (`events`).
    pub fn events(&mut self, since: u64) -> Result<(Vec<Value>, u64), Error> {
        let v = self.call("events", json!({ "since": since }))?;
        let list = v["events"].as_array().cloned().unwrap_or_default();
        Ok((list, v["last"].as_u64().unwrap_or(since)))
    }

    /// The undo history: where it stands and its steps (`history`).
    pub fn history(&mut self) -> Result<Value, Error> {
        self.call("history", json!({}))
    }

    /// The changes waiting to be accepted or discarded (`proposals`).
    pub fn proposals(&mut self) -> Result<Value, Error> {
        self.call("proposals", json!({}))
    }

    /// The parameters of the plugin playing channel or slot `owner` (`params`).
    pub fn params(&mut self, owner: u32) -> Result<Value, Error> {
        self.call("params", json!({ "owner": owner }))
    }
}
