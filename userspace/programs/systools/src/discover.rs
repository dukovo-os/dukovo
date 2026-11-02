// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! discover(1): what the local networks have, as discoverd(8) finds it
//! (ADR 0338): the services their machines offer by DNS-SD, and the
//! devices that answer WS-Discovery, as Windows machines and many
//! printers and scanners do.
//!
//! `discover [-w SECONDS] [-r] [TYPE...]` looks for SECONDS (3 unless
//! said) for services of each TYPE, as `_ipp._tcp`, or of every type the
//! networks say they have, and prints a table of them, then of the
//! devices; `-r` prints discoverd's lines as they are, by tabs.

use std::io::{self, Write};
use std::time::Duration;

use crate::args::{Arg, Args};
use crate::discoverd;
use crate::util::{self, Fail, fail, usage};

/// How long it looks, unless `-w` says.
const WAIT: Duration = Duration::from_secs(3);

/// The longest it may look.
const WAIT_MAX: Duration = Duration::from_secs(30);

/// `text`, a number of seconds to look for, up to WAIT_MAX.
fn seconds(text: &str) -> Result<Duration, Fail> {
    text.parse::<f64>()
        .ok()
        .and_then(|s| Duration::try_from_secs_f64(s).ok())
        .filter(|w| *w <= WAIT_MAX)
        .ok_or_else(|| usage(format!("{text}: not a number of seconds up to 30")))
}

/// discover(1).
pub fn main(args: &mut Args) -> Result<i32, Fail> {
    let mut wait = WAIT;
    let mut raw = false;
    let mut types = Vec::new();
    while let Some(arg) = args.next()? {
        match arg {
            Arg::Short('w') => wait = seconds(&args.text()?)?,
            Arg::Long(l) if l == "wait" => wait = seconds(&args.text()?)?,
            Arg::Short('r') => raw = true,
            Arg::Long(l) if l == "raw" => raw = true,
            Arg::Operand(ty) => {
                let ty = ty.to_string_lossy().into_owned();
                if !ty.starts_with('_') || ty.contains(char::is_whitespace) {
                    return Err(usage(format!("{ty}: a type is as `_ipp._tcp`")));
                }
                types.push(ty);
            }
            other => return Err(args.unknown(&other)),
        }
    }
    let mut services: Vec<Vec<String>> = Vec::new();
    let mut devices: Vec<Vec<String>> = Vec::new();
    let mut out = io::stdout().lock();
    let mut failed = None;
    let result = discoverd::browse_lines(wait, &types, |line| {
        if raw {
            if let Err(e) = writeln!(out, "{line}") {
                failed.get_or_insert(e);
            }
            return;
        }
        let mut fields = line.split('\t');
        match fields.next() {
            Some("service") => services.push(fields.map(str::to_string).collect()),
            Some("device") => devices.push(fields.map(str::to_string).collect()),
            _ => {}
        }
    });
    result.map_err(|e| {
        if matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused) {
            fail("discoverd is not running".into())
        } else {
            util::failed(&discoverd::SOCKET, e)
        }
    })?;
    if let Some(e) = failed {
        return Err(e.into());
    }
    if raw {
        return Ok(0);
    }
    services.sort();
    devices.sort();
    if services.is_empty() && devices.is_empty() {
        writeln!(out, "Nothing answered in {:.1}s.", wait.as_secs_f64())?;
        return Ok(0);
    }
    if !services.is_empty() {
        let rows: Vec<Vec<String>> = services
            .iter()
            .map(|s| {
                let field = |i: usize| s.get(i).cloned().unwrap_or_default();
                let host = field(2);
                vec![field(0), field(1), format!("{host}:{}", field(3)), field(4)]
            })
            .collect();
        table(&mut out, &["SERVICE", "TYPE", "AT", "ADDRESSES"], &rows)?;
    }
    if !devices.is_empty() {
        if !services.is_empty() {
            writeln!(out)?;
        }
        let rows: Vec<Vec<String>> = devices
            .iter()
            .map(|d| {
                let field = |i: usize| d.get(i).cloned().unwrap_or_default();
                vec![field(2), field(1), field(0), field(3)]
            })
            .collect();
        table(&mut out, &["DEVICE", "TYPES", "ENDPOINT", "DESCRIBED AT"], &rows)?;
    }
    Ok(0)
}

/// Writes `rows` under `heads`, each column as wide as its widest cell;
/// the last as it is.
fn table(out: &mut impl Write, heads: &[&str], rows: &[Vec<String>]) -> io::Result<()> {
    let mut widths: Vec<usize> = heads.iter().map(|h| util::width(h)).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if let Some(width) = widths.get_mut(i) {
                *width = (*width).max(util::width(cell));
            }
        }
    }
    let line = |out: &mut dyn Write, cells: &mut dyn Iterator<Item = &str>| -> io::Result<()> {
        let mut text = String::new();
        for (i, cell) in cells.enumerate() {
            if i + 1 == widths.len() {
                text.push_str(cell);
            } else {
                text.push_str(cell);
                let pad = widths[i].saturating_sub(util::width(cell)) + 2;
                text.extend(std::iter::repeat_n(' ', pad));
            }
        }
        writeln!(out, "{}", text.trim_end())
    };
    line(out, &mut heads.iter().copied())?;
    for row in rows {
        line(out, &mut row.iter().map(String::as_str))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_table_lines_its_columns_up() {
        let mut out = Vec::new();
        let rows = vec![
            vec![
                "b".to_string(),
                "_workstation._tcp".into(),
                "b.local:9".into(),
                "10.0.2.16".into(),
            ],
            vec!["printer".to_string(), "_ipp._tcp".into(), "p.local:631".into(), String::new()],
        ];
        table(&mut out, &["SERVICE", "TYPE", "AT", "ADDRESSES"], &rows).unwrap();
        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "SERVICE  TYPE               AT           ADDRESSES");
        assert_eq!(lines[1], "b        _workstation._tcp  b.local:9    10.0.2.16");
        assert_eq!(lines[2], "printer  _ipp._tcp          p.local:631");
    }
}
