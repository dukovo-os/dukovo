// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `POST /mcp`: the console's MCP server over HTTPS (ADRs 0690 and
//! 0692).
//!
//! One JSON-RPC message, or for the oldest revisions one batch, comes in
//! the body, and one JSON answer goes back: no session, no stream, and
//! `GET` and `DELETE` are not used here (405). The client's token comes
//! with every request, as `Authorization: Bearer dkc_...`, and was
//! checked with authd before this is reached. An answer's status is the
//! protocol's ([`mcp::protocol`](crate::admind::mcp::protocol)); a
//! notification is answered 202 with no body.

use http::StatusCode;
use http::header::HeaderName;

use super::{Content, Context, Outcome, Reply};
use crate::admind::errors::{Failure, Kind};
use crate::admind::mcp::client::Client;
use crate::admind::mcp::protocol::Headers;
use crate::admind::mcp::server;

/// The value of the header `name`, if the request has it once.
fn once<'a>(cx: &'a Context<'_>, name: &'static str) -> Result<Option<&'a [u8]>, Failure> {
    let mut values = cx.headers.get_all(HeaderName::from_static(name)).iter();
    let first = values.next().map(http::HeaderValue::as_bytes);
    if values.next().is_some() {
        let why = format!("a request has the header {name} once");
        return Err(Failure::new(Kind::BadRequest, why));
    }
    Ok(first)
}

/// The refusal of a request to `/mcp` that brings no client's token.
pub fn no_token() -> Failure {
    let why = "this address takes an AI client's token, as Authorization: Bearer dkc_…";
    Failure::new(Kind::ClientRefused, why)
}

/// Answers `POST /mcp` for `client`, which the request's token named.
pub async fn post(cx: &Context<'_>, client: Option<Client>) -> Outcome {
    let client = client.ok_or_else(no_token)?;
    let headers = Headers {
        version: once(cx, "mcp-protocol-version")?,
        method: once(cx, "mcp-method")?,
        name: once(cx, "mcp-name")?,
    };
    let answered = server::handle(cx.shared, &client, &cx.body, Some(&headers)).await;
    let status = StatusCode::from_u16(answered.status).unwrap_or(StatusCode::OK);
    let content = answered.body.map_or(Content::Nothing, Content::Json);
    Ok(Reply { status, content, cookies: Vec::new(), retry_after: None })
}
