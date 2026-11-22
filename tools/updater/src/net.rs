// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! HTTPS: rustls (with ring), checked against the system's certificate store.

use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

use sha2::{Digest, Sha256};
use ureq::tls::{Certificate, RootCerts, TlsConfig, TlsProvider};

pub struct Net {
    agent: ureq::Agent,
}

impl Net {
    pub fn new() -> Result<Net, String> {
        // The one crypto provider built in; an error only means one is installed already.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let found = rustls_native_certs::load_native_certs();
        let certs: Vec<Certificate<'static>> = found.certs.iter().map(|c| Certificate::from_der(c.as_ref()).to_owned()).collect();
        if certs.is_empty() {
            let why = found.errors.first().map(|e| format!(": {e}")).unwrap_or_default();
            return Err(format!("no certificates in the system's certificate store{why}"));
        }
        let tls = TlsConfig::builder().provider(TlsProvider::Rustls).root_certs(RootCerts::new_with_certs(&certs)).build();
        let config = ureq::Agent::config_builder()
            .tls_config(tls)
            .user_agent(format!("lemonstudio-updater/{}", env!("CARGO_PKG_VERSION")))
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            .timeout_per_read(Some(Duration::from_secs(60)))
            .build();
        Ok(Net { agent: config.into() })
    }

    /// The body at `url` (redirects followed); None when there is nothing there (404).
    pub fn get(&self, url: &str) -> Result<Option<Vec<u8>>, String> {
        match self.agent.get(url).call() {
            Ok(mut r) => r.body_mut().with_config().limit(16 << 20).read_to_vec().map(Some).map_err(|e| format!("{url}: {e}")),
            Err(ureq::Error::StatusCode(404)) => Ok(None),
            Err(e) => Err(format!("{url}: {e}")),
        }
    }

    /// Downloads `url` to `to`, which must then be `size` bytes with this SHA-256 (lowercase hex); `progress` hears
    /// (bytes so far, size) now and then. A file that does not match is deleted.
    pub fn download(&self, url: &str, to: &Path, size: u64, sha256: &str, mut progress: impl FnMut(u64, u64)) -> Result<(), String> {
        let r = self.agent.get(url).call().map_err(|e| format!("{url}: {e}"))?;
        let mut body = r.into_body().into_reader();
        let mut file = std::fs::File::create(to).map_err(|e| format!("{}: {e}", to.display()))?;
        let mut hash = Sha256::new();
        let (mut done, mut told) = (0u64, 0u64);
        let mut buf = vec![0u8; 1 << 16];
        let result = loop {
            let n = match body.read(&mut buf) {
                Ok(0) => break Ok(()),
                Ok(n) => n,
                Err(e) => break Err(format!("{url}: {e}")),
            };
            done += n as u64;
            if done > size {
                break Err(format!("{url}: larger than the manifest says ({size} bytes)"));
            }
            hash.update(&buf[..n]);
            if let Err(e) = file.write_all(&buf[..n]) {
                break Err(format!("{}: {e}", to.display()));
            }
            if done - told >= 1 << 20 {
                told = done;
                progress(done, size);
            }
        };
        let result = result
            .and_then(|()| file.sync_all().map_err(|e| format!("{}: {e}", to.display())))
            .and_then(|()| if done == size { Ok(()) } else { Err(format!("{url}: {done} bytes, the manifest says {size}")) })
            .and_then(|()| {
                let got = hex(&hash.finalize());
                if got.eq_ignore_ascii_case(sha256) { Ok(()) } else { Err(format!("{url}: SHA-256 {got} does not match the manifest's")) }
            });
        drop(file);
        if result.is_err() {
            let _ = std::fs::remove_file(to);
        } else {
            progress(done, size);
        }
        result
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
