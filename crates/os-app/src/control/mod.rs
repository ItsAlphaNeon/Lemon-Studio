// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! `os_session::control` with the children that stay with the interface (control-plane step 9).

pub use os_session::control::*;

pub(crate) mod server_ui;
pub(crate) mod rpc_ui;
pub(crate) mod host_ui;
pub(crate) mod query_ui;
#[cfg(test)]
mod tests;
