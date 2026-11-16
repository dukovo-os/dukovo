// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! database(8): PostgreSQL's databases, made, opened to other machines
//! and removed, for the web console and for people at a terminal.
//!
//! [`wire`] speaks PostgreSQL's protocol over the server's socket, as the
//! role postgres that the server lets root be, and [`scram`] makes a
//! role's password and the verifier PostgreSQL keeps in its place, so
//! that the password never reaches the server.

// The command that uses these is written beside them.
#![allow(dead_code)]

pub mod scram;
pub mod wire;
