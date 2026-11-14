// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Problem reports (ADRs 0530 to 0532): reportd(8) keeps the crashes of
//! programs, services and the kernel, drafts and redacts reports, and
//! queues and sends the ones a person approved; report(1) asks the
//! questions, shows the exact bytes, and sends, saves or hands them to a
//! phone.
//!
//! - [`protocol`] is what goes over reportd's socket, a line of JSON each
//!   way;
//! - [`facts`] are the machine's and the system's, which open a report;
//! - [`redact`] cuts what names a person or a machine out of a text;
//! - [`draft`] puts the text of a report together;
//! - [`client`] asks reportd, with a time limit;
//! - [`command`] is `report(1)`;
//! - [`preview`] shows the text and reads the key that says what to do
//!   with it;
//! - [`save`] writes the text to a file, and serves it once over HTTPS to
//!   a phone;
//! - [`qr`] draws a QR code in half blocks, for that download's address
//!   and for the failure notice's support page (login(1)).

pub mod client;
pub mod command;
pub mod draft;
pub mod facts;
pub mod preview;
pub mod protocol;
pub mod qr;
pub mod redact;
pub mod save;
