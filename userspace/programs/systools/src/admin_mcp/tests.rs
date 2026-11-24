// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::ffi::OsString;

use super::*;

fn args(items: &[&str]) -> Args {
    Args::new("admin", items.iter().map(OsString::from).collect())
}

/// The mistake in the command line that `admin mcp ITEMS` is told.
fn mistake(items: &[&str]) -> String {
    let fail = main(&mut args(items)).expect_err("a mistake");
    assert!(fail.usage, "{items:?}: {}", fail.message);
    fail.message
}

/// Noon on 2027-01-15 in UTC, which is that day in every time zone
/// within eleven hours of it.
const NOON: i64 = 1_800_014_400;

fn listed(name: &str, way: &str, scopes: &[&str]) -> Listed {
    Listed {
        owner: "chase".into(),
        name: name.into(),
        way: way.into(),
        scopes: scopes.iter().map(|scope| (*scope).to_string()).collect(),
        until: NOON,
        used: 0,
        from: String::new(),
        ended: false,
    }
}

#[test]
fn a_mistake_in_the_command_line_is_told_before_authd_is_asked_anything() {
    assert!(
        mistake(&[]).starts_with("which? add, list, remove, or a client's name\nusage: admin mcp")
    );
    assert!(mistake(&["--check"]).starts_with("which?"));
    // add: a name, a scope, and nothing it does not know.
    assert_eq!(mistake(&["add"]), "add takes the client's name and its scopes");
    assert!(mistake(&["add", "desk"]).starts_with("add takes at least one --scope"));
    assert_eq!(mistake(&["add", "desk", "--scope"]), "an option needs a value");
    assert!(mistake(&["add", "desk", "laptop", "--scope", "*"]).contains("laptop"));
    assert!(mistake(&["add", "desk", "--scope", "*", "--weeks", "2"]).contains("--weeks"));
    assert!(mistake(&["add", "desk", "--scope", "*", "--days", "many"]).contains("many"));
    // A name that is one of the command's own words, or is no name.
    for name in ["list", "remove", "help", "Desk", "a b", "desk_1"] {
        let told = mistake(&["add", name, "--scope", "*"]);
        assert!(told.starts_with("an AI client's name is 1 to 32 of a to z"), "{name}: {told}");
    }
    assert!(mistake(&["add", "desk", "--scope", "services:write"]).contains("is not a scope"));
    assert!(mistake(&["add", "desk", "--scope", "service"]).contains("there is no area service."));
    // list and remove.
    assert!(mistake(&["list", "mine"]).contains("mine"));
    assert!(mistake(&["list", "--yaml"]).contains("--yaml"));
    assert_eq!(mistake(&["remove"]), "remove takes the name of an AI client");
    assert!(mistake(&["remove", "a", "b"]).contains('b'));
    assert_eq!(mistake(&["remove", "--of"]), "an option needs a value");
    // A client's name, to connect it.
    assert!(mistake(&["desk", "--now"]).contains("--now"));
    assert!(mistake(&["desk", "--check", "again"]).contains("again"));
    assert!(mistake(&["Desk"]).starts_with("Desk is not an AI client's name"));
}

#[test]
fn a_scope_is_an_area_this_console_has_alone_or_to_read_or_every_area() {
    let of = |values: &[&str]| {
        let values: Vec<String> = values.iter().map(|value| (*value).to_string()).collect();
        scopes_of(&values)
    };
    assert_eq!(of(&["services"]).unwrap(), ["services"]);
    // One by one or with commas, in the order given.
    assert_eq!(of(&["services,logs:read", "*:read"]).unwrap(), ["services", "logs:read", "*:read"]);
    assert_eq!(of(&["*"]).unwrap(), ["*"]);
    for wrong in ["", "services,", "Services", "services:write", "*:all", "logs:read:read"] {
        let fail = of(&[wrong]).expect_err(wrong);
        assert!(fail.usage && fail.message.contains("is not a scope"), "{wrong}: {}", fail.message);
    }
    // An area there is not would reach nothing, and say nothing of it.
    let none = of(&["services", "service:read"]).expect_err("no such area");
    assert!(none.message.starts_with("service:read is not a scope: there is no area service."));
    assert!(none.message.contains("The areas are services, logs, "), "{}", none.message);
    // Every area the console's clients are told of can be given.
    for (area, _) in AREAS {
        assert!(of(&[area, &format!("{area}:read")]).is_ok(), "{area}");
    }
}

#[test]
fn what_only_an_administrator_inside_elevate_asks_is_said_as_the_command_that_works() {
    let asked = inside_elevate("lists every account's AI clients", "admin mcp list --all");
    if util::own_user() == 0 {
        assert!(asked.is_ok());
        return;
    }
    let fail = asked.expect_err("not root");
    assert!(!fail.usage);
    assert_eq!(
        fail.message,
        "only an administrator lists every account's AI clients: run it with elevate, as \
         `elevate admin mcp list --all`"
    );
    // Said before authd is asked, with the client and its owner in it.
    let fail = main(&mut args(&["remove", "desk", "--of", "alice"])).expect_err("not root");
    assert!(
        fail.message.ends_with("`elevate admin mcp remove desk --of alice`"),
        "{}",
        fail.message
    );
    assert!(main(&mut args(&["list", "--all", "--json"])).is_err());
}

#[test]
fn the_clients_are_a_table_of_how_each_connects_what_it_reaches_and_when_it_ends() {
    let text = serde_json::json!({ "clients": [
        { "owner": "chase", "name": "desk", "way": "ssh", "scopes": ["services", "logs:read"],
          "until": NOON, "made": 1_790_000_000, "used": 0, "from": "", "ended": false },
        { "owner": "alice", "name": "watch", "way": "https", "scopes": ["*:read"],
          "until": NOON, "made": 1_790_000_000, "used": NOON, "from": "192.0.2.7", "ended": true },
    ] })
    .to_string();
    let clients = clients_in(&text).unwrap();
    assert_eq!(clients.len(), 2);
    let used = util::local_time(NOON, "%Y-%m-%d %H:%M");
    assert_eq!(
        clients_table(&clients, true),
        format!(
            "OWNER  NAME   CONNECTS    REACHES              ENDS                LAST USED\n\
             chase  desk   over SSH    services, logs:read  2027-01-15          never\n\
             alice  watch  over HTTPS  *:read               2027-01-15 (ended)  {used} from 192.0.2.7\n"
        )
    );
    // One's own are listed without their owner.
    let own = clients_table(&clients[..1], false);
    assert_eq!(
        own,
        "NAME  CONNECTS  REACHES              ENDS        LAST USED\n\
         desk  over SSH  services, logs:read  2027-01-15  never\n"
    );
    // Used, from where authd was not told.
    let quiet = Listed { used: NOON, ..listed("desk", "ssh", &["*"]) };
    assert!(clients_table(&[quiet], false).ends_with(&format!("{used}\n")));
    for wrong in ["", "{}", "{\"clients\": [{\"name\": \"desk\"}]}"] {
        let fail = clients_in(wrong).expect_err(wrong);
        assert_eq!(fail.message, "authd answered a list of AI clients that does not read");
    }
}

#[test]
fn a_client_let_in_over_ssh_is_told_the_command_its_settings_run() {
    let told = connects_over_ssh("chase", "desk");
    let command = format!("  ssh chase@{} admin mcp desk\n", util::hostname());
    assert!(told.contains(&command), "{told}");
    assert!(told.starts_with("The AI client desk is let in."), "{told}");
    assert!(told.contains("waits for an administrator's yes"), "{told}");
}

#[test]
fn the_address_an_ssh_session_came_from_is_its_first_word_if_it_is_an_address() {
    assert_eq!(came_from("192.0.2.7 50122 198.51.100.4 22"), "192.0.2.7");
    assert_eq!(came_from("2001:db8::7 50122 2001:db8::1 22"), "2001:db8::7");
    for none in ["", "   ", "laptop 50122 server 22", "192.0.2.7;rm 1 2 3", "999.0.2.7 1 2 3"] {
        assert_eq!(came_from(none), "", "{none:?}");
    }
}

#[test]
fn the_console_s_greeting_takes_the_pass_or_says_why_not() {
    assert_eq!(greeted("ok chase desk"), Ok(()));
    assert_eq!(
        greeted("no ADM-143: this pass is alice's, and is taken from that account alone"),
        Err("ADM-143: this pass is alice's, and is taken from that account alone".into())
    );
    for unread in ["", "ok", "hello there", "{\"jsonrpc\":\"2.0\"}"] {
        assert_eq!(greeted(unread), Err("the console's greeting does not read".into()), "{unread}");
    }
}

#[test]
fn a_line_is_read_to_its_end_and_no_longer_than_it_may_be() {
    let read = |text: &str, limit| line_from(&mut text.as_bytes(), limit);
    assert_eq!(read("ok chase desk\n{\"id\":1}\n", 64).unwrap(), b"ok chase desk");
    assert_eq!(read("\nrest", 64).unwrap(), b"");
    assert_eq!(read("12345\n", 5).unwrap(), b"12345");
    assert_eq!(read("123456\n", 5).unwrap_err().kind(), io::ErrorKind::InvalidData);
    assert_eq!(read("no end", 64).unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
    assert_eq!(read("", 64).unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
}

#[test]
fn a_check_says_what_the_client_reaches_or_what_the_console_refused() {
    let answered = |result: Value| json!({ "jsonrpc": "2.0", "id": 1, "result": result });
    let areas = answered(json!({ "isError": false, "structuredContent": { "areas": [
        { "area": "services", "title": "Services", "reach": "read all and ask", "reads": 12,
          "changes": 30 },
        { "area": "logs", "title": "Logs", "reach": "read", "reads": 2, "changes": 0 },
    ] } }));
    assert_eq!(
        reaches("desk", &areas).unwrap(),
        "The AI client desk connects. It reaches:\n\
         AREA      REACH             READS  CHANGES TO ASK FOR\n\
         services  read all and ask  12     30\n\
         logs      read              2      0\n"
    );
    let nothing = answered(json!({ "isError": false, "structuredContent": { "areas": [] } }));
    let said = reaches("desk", &nothing).unwrap();
    assert!(said.starts_with("The AI client desk connects, and reaches nothing"), "{said}");
    // A tool's failure, with the console's ID; the protocol's error; and
    // an answer that is neither.
    let failed = answered(json!({ "isError": true, "structuredContent": { "error": {
        "id": "ADM-105", "title": "Wait", "detail": "this AI client made 120 calls in a minute",
    } } }));
    assert_eq!(
        reaches("desk", &failed),
        Err("ADM-105: this AI client made 120 calls in a minute".into())
    );
    let refused =
        json!({ "jsonrpc": "2.0", "id": 1, "error": { "code": -32603, "message": "no" } });
    assert_eq!(reaches("desk", &refused), Err("no".into()));
    for unread in [json!({}), answered(json!({})), answered(json!({ "structuredContent": 7 }))] {
        assert_eq!(reaches("desk", &unread), Err("the console's answer does not read".into()));
    }
}
