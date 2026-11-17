// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Showing an action (ADR 0515): what an action with its arguments would
//! run, in the forms a person copies, and nothing run. `POST /api/1/show`
//! and `admin show` (admin(8)) both answer with [`shown`], so that the
//! page and the command say the same.
//!
//! - the command, as one line for Dukovo's shell and for sh, with
//!   `elevate` in front when the action runs as an administrator, and
//!   what is typed when the command asks, for one that reads a secret;
//! - the API call, as a `curl` line against the console's address;
//! - the Ansible task, as YAML text;
//! - the files a change writes.
//!
//! The command is the argv that runs, quoted ([`shell_word`]); the other
//! forms are made from the same prepared action, so none can say what
//! another does not.
//!
//! A secret is never shown, and is not needed to show an action (ADR
//! 0561): [`prepare`] puts `(secret)` in its place. The command says that
//! it is typed when asked, the `curl` line takes it from a variable of the
//! field's name in capitals (`$PASSWORD`), and the Ansible task from the
//! playbook's (`{{ password }}`), with `no_log`. A document that is a
//! secret, as a certificate's key is (ADR 0567), is given on the command's
//! input, and the `curl` line takes it from a variable that holds it as
//! JSON text (`$PEM_JSON`), since it has line ends.

use serde_json::{Map, Value, json};

use super::secret::SHOWN;
use super::{Ansible, Field, Prepared, Registered, Runs, Type};
use crate::admind::errors::Failure;

/// The action `id` in `actions`.
///
/// # Errors
///
/// ADM-125 for the field `action`, when there is none.
pub fn lookup(
    actions: &'static [&'static dyn Registered],
    id: &str,
) -> Result<&'static dyn Registered, Failure> {
    actions.iter().copied().find(|action| action.id() == id).ok_or_else(|| {
        Failure::bad_value(
            "action",
            format!("There is no action {id}: admin actions lists them. Nothing was done."),
        )
    })
}

/// `action` prepared with `args` for showing: each secret field that
/// `args` gives, and each that is required, holds `(secret)` in place of
/// its value, so that a page need not send a secret to show the command
/// and none that is sent is kept. A page gives an optional secret that
/// will be typed as empty text, which says that it is given.
///
/// # Errors
///
/// As [`Registered::prepare`].
pub fn prepare(action: &'static dyn Registered, args: Value) -> Result<Prepared, Failure> {
    let mut args = args;
    if let Value::Object(given) = &mut args {
        for field in action.fields().iter().filter(|field| field.is_secret()) {
            let present = given.get(field.name).is_some_and(|value| !value.is_null());
            if present || field.required {
                given.insert(field.name.into(), SHOWN.into());
            }
        }
    }
    action.prepare(args)
}

/// What `prepared` shows as: `command`; `typed`, what is typed when the
/// command asks, or null; `api` for the console at `origin`
/// (`https://host:port`); `ansible`; and `files`.
pub fn shown(prepared: &Prepared, origin: &str) -> Value {
    json!({
        "command": command(prepared),
        "typed": typed(prepared),
        "api": curl(prepared, origin),
        "ansible": yaml(&prepared.ansible),
        "files": prepared.files,
    })
}

/// The fields that `prepared`'s arguments give and its command reads on
/// its input, in the order of the fields: its secrets and its documents.
fn inputs_given(prepared: &Prepared) -> Vec<&'static Field> {
    prepared
        .action
        .fields()
        .iter()
        .filter(|field| field.is_input())
        .filter(|field| prepared.args.get(field.name).is_some_and(|value| !value.is_null()))
        .collect()
}

/// The secrets that `prepared`'s arguments give, as the `curl` line takes
/// them from variables: each name, and whether it is a document.
fn secrets_given(prepared: &Prepared) -> Vec<(&'static str, bool)> {
    inputs_given(prepared)
        .into_iter()
        .filter(|field| field.is_secret())
        .map(|field| (field.name, matches!(field.kind, Type::Document { .. })))
        .collect()
}

/// What a person gives the command as it runs: "then the password, typed
/// when asked", and "with the pem on its input" for a document. None for
/// a command that reads neither.
pub fn typed(prepared: &Prepared) -> Option<String> {
    let given = inputs_given(prepared);
    let names = |documents: bool| -> Option<String> {
        let names: Vec<String> = given
            .iter()
            .filter(|field| matches!(field.kind, Type::Document { .. }) == documents)
            .map(|field| format!("the {}", field.name.replace('_', " ")))
            .collect();
        match names.as_slice() {
            [] => None,
            [one] => Some(one.clone()),
            [rest @ .., last] => Some(format!("{} and {last}", rest.join(", "))),
        }
    };
    match (names(false), names(true)) {
        (None, None) => None,
        (Some(lines), None) => Some(format!("then {lines}, typed when asked")),
        (None, Some(documents)) => Some(format!("with {documents} on its input")),
        (Some(lines), Some(documents)) => {
            Some(format!("then {lines}, typed when asked, and {documents} on its input"))
        }
    }
}

/// `word` as one word of a line for sh. A word of `A-Za-z0-9@%+=:,./_-`
/// stays as it is; any other, and the empty word, goes in single quotes,
/// where only a single quote needs more: it is written `'\''`, which ends
/// the quotes, quotes the quote and opens them again.
pub fn shell_word(word: &str) -> String {
    let plain = !word.is_empty()
        && word.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"@%+=:,./_-".contains(&byte));
    if plain { word.to_string() } else { format!("'{}'", word.replace('\'', "'\\''")) }
}

/// The command as one line, which an administrator at a terminal needs
/// `elevate` for.
pub fn command(prepared: &Prepared) -> String {
    let words: Vec<String> = prepared.argv.iter().map(|word| shell_word(word)).collect();
    let line = words.join(" ");
    match prepared.action.runs() {
        Runs::AsAdministrator => format!("elevate {line}"),
        Runs::AsThePerson => line,
    }
}

/// The `curl` line that asks the console at `origin` for the action, as a
/// script with a token does. A read's arguments that the address does not
/// hold go in its query, a change's in its body, as do those of a read that
/// takes or answers a secret. A secret is the shell's variable of its
/// field's name in capitals, inside a body in double quotes.
fn curl(prepared: &Prepared, origin: &str) -> String {
    let action = prepared.action;
    let (path, mut rest) = address(action.path(), &prepared.args);
    let bearer = r#"-H "Authorization: Bearer $DUKOVO_TOKEN""#;
    if !action.posts() {
        let url = shell_word(&format!("{origin}{path}{}", query(&rest)));
        return format!("curl -sS {bearer} {url}");
    }
    let url = shell_word(&format!("{origin}{path}"));
    let secrets = secrets_given(prepared);
    let body = if secrets.is_empty() {
        shell_word(&Value::Object(rest).to_string())
    } else {
        for (name, _) in &secrets {
            rest.remove(*name);
        }
        body_with_variables(&rest, &secrets)
    };
    let json = r#"-H "Content-Type: application/json""#;
    format!("curl -sS -X POST {bearer} {json} -d {body} {url}")
}

/// The JSON object of `plain` and of each of `secrets` as the shell's
/// variable of its name in capitals, as one word in double quotes for sh:
/// `"{\"name\":\"bob\",\"password\":\"$PASSWORD\"}"`. The variable's
/// value goes in as it is, so it is text that a JSON string holds as it is.
/// A document's variable (`true` beside its name) is its name in capitals
/// and `_JSON`, outside quotes: it holds the document as a JSON string,
/// quotes and escapes and all, as `jq -Rs .` makes one of a file.
pub(super) fn body_with_variables(plain: &Map<String, Value>, secrets: &[(&str, bool)]) -> String {
    let text = Value::Object(plain.clone()).to_string();
    // `{...}` without its closing brace, to which the secrets are added.
    let open = &text[..text.len() - 1];
    let mut word = format!("\"{}", double_quoted(open));
    for (at, (name, document)) in secrets.iter().enumerate() {
        if at > 0 || !plain.is_empty() {
            word.push(',');
        }
        let variable = name.to_ascii_uppercase();
        if *document {
            let key = double_quoted(&format!("{}:", Value::from(*name)));
            word.push_str(&format!("{key}${variable}_JSON"));
        } else {
            let key = double_quoted(&format!("{}:\"", Value::from(*name)));
            word.push_str(&format!("{key}${variable}\\\""));
        }
    }
    word.push_str("}\"");
    word
}

/// `text` as it is written inside double quotes for sh: `\`, `"`, `$` and
/// `` ` `` each with a backslash in front.
fn double_quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '\\' | '"' | '$' | '`') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The address of `template` with each `{name}` part filled in from `args`
/// and percent-encoded, and the arguments that the address does not
/// hold, which a body carries.
fn address(template: &str, args: &Value) -> (String, Map<String, Value>) {
    let mut rest = args.as_object().cloned().unwrap_or_default();
    let parts: Vec<String> = template
        .split('/')
        .map(|part| {
            let name = part.strip_prefix('{').and_then(|inner| inner.strip_suffix('}'));
            match name.and_then(|name| rest.remove(name)) {
                Some(Value::String(text)) => percent_encode(&text),
                Some(other) => percent_encode(&other.to_string()),
                None => part.to_string(),
            }
        })
        .collect();
    (parts.join("/"), rest)
}

/// `args` as an address's query, `?name=value&…` with each part
/// percent-encoded, or nothing when there are none. An argument that is
/// not given (null) is left out.
fn query(args: &Map<String, Value>) -> String {
    let pairs: Vec<String> = args
        .iter()
        .filter(|(_, value)| !value.is_null())
        .map(|(name, value)| {
            let value = match value {
                Value::String(text) => percent_encode(text),
                other => percent_encode(&other.to_string()),
            };
            format!("{}={value}", percent_encode(name))
        })
        .collect();
    if pairs.is_empty() { String::new() } else { format!("?{}", pairs.join("&")) }
}

/// `text` with every byte outside `A-Za-z0-9-._~` written as `%XX`.
fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// `task` as the YAML of a play's task list with the one task: its name,
/// its module with the parameters, `become: true` when it runs as root,
/// and `register`, `changed_when`, `failed_when` and `when` when the task says;
/// then, when the machine restarts after it, [`COMES_BACK`].
/// Each string is written as a JSON
/// string, which YAML reads as the same string; a list is a block list,
/// and a list or mapping inside one is written as JSON, which is YAML's
/// flow style.
pub fn yaml(task: &Ansible) -> String {
    let mut out = format!("- name: {}\n  {}:\n", Value::from(task.name.as_str()), task.module);
    for (key, value) in &task.params {
        parameter(&mut out, 4, key, value);
    }
    if task.become_root {
        out.push_str("  become: true\n");
    }
    if task.no_log {
        out.push_str("  no_log: true\n");
    }
    if let Some(name) = task.register {
        out.push_str(&format!("  register: {name}\n"));
    }
    if let Some(when) = &task.changed_when {
        let when = match when.as_str() {
            "true" | "false" => when.clone(),
            condition => Value::from(condition).to_string(),
        };
        out.push_str(&format!("  changed_when: {when}\n"));
    }
    if let Some(when) = &task.failed_when {
        out.push_str(&format!("  failed_when: {}\n", Value::from(when.as_str())));
    }
    if let Some(when) = &task.when {
        out.push_str(&format!("  when: {}\n", Value::from(when.as_str())));
    }
    if task.comes_back {
        out.push_str(COMES_BACK);
    }
    out
}

/// The task after one that restarts the machine (ADR 0565). The restart
/// comes a minute after the command answers, so the play waits two
/// minutes before it tries to connect, which the machine is then going
/// down or starting for, then up to half an hour for its firmware, its
/// disks and an update's tries.
const COMES_BACK: &str = "- name: \"Wait for the machine to answer again\"\n  \
                          ansible.builtin.wait_for_connection:\n    delay: 120\n    \
                          timeout: 1800\n";

/// Writes the parameter `key` of `value` at `indent` spaces: a list or a
/// mapping with something in it on the lines below, anything else on the
/// line.
fn parameter(out: &mut String, indent: usize, key: &str, value: &Value) {
    let pad = " ".repeat(indent);
    let key = yaml_key(key);
    match value {
        Value::Array(items) if !items.is_empty() => {
            out.push_str(&format!("{pad}{key}:\n"));
            for item in items {
                out.push_str(&format!("{pad}  - {item}\n"));
            }
        }
        Value::Object(map) if !map.is_empty() => {
            out.push_str(&format!("{pad}{key}:\n"));
            for (inner, value) in map {
                parameter(out, indent + 2, inner, value);
            }
        }
        scalar => out.push_str(&format!("{pad}{key}: {scalar}\n")),
    }
}

/// `key` as a mapping's key: as it is when it is a name, else a JSON
/// string.
fn yaml_key(key: &str) -> String {
    let name = key.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && key.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
    if name { key.to_string() } else { Value::from(key).to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admind::actions::REGISTRY;
    use crate::admind::errors::Kind;

    const ORIGIN: &str = "https://nas.example:9090";

    /// What the action `id` shows for `args`, at [`ORIGIN`].
    fn show(id: &str, args: Value) -> Value {
        let action = lookup(REGISTRY, id).unwrap();
        shown(&action.prepare(args).unwrap(), ORIGIN)
    }

    #[test]
    fn a_word_of_plain_characters_stays_as_it_is() {
        for word in [
            "service",
            "nginx",
            "--json",
            "-n",
            "a=b",
            "a,b:c",
            "/system/commands/x",
            "a@b.c",
            "100%",
            "x+y",
            "a_b-c.d",
        ] {
            assert_eq!(shell_word(word), word);
        }
        assert_eq!(shell_word("-"), "-");
    }

    #[test]
    fn any_other_word_goes_in_single_quotes() {
        assert_eq!(shell_word(""), "''");
        assert_eq!(shell_word("my site"), "'my site'");
        assert_eq!(shell_word("$HOME"), "'$HOME'");
        assert_eq!(shell_word("`id`"), "'`id`'");
        assert_eq!(shell_word(r"a\b"), r"'a\b'");
        assert_eq!(shell_word("a\nb"), "'a\nb'");
        assert_eq!(shell_word("a\tb"), "'a\tb'");
        assert_eq!(shell_word("caf\u{e9}"), "'caf\u{e9}'");
        assert_eq!(shell_word("a;b"), "'a;b'");
        assert_eq!(shell_word("a&b|c"), "'a&b|c'");
        assert_eq!(shell_word("a\"b"), "'a\"b'");
        assert_eq!(shell_word("~"), "'~'");
        assert_eq!(shell_word("*"), "'*'");
        assert_eq!(shell_word("!x"), "'!x'");
        assert_eq!(shell_word("#x"), "'#x'");
        assert_eq!(shell_word("{}"), "'{}'");
    }

    #[test]
    fn a_single_quote_ends_the_quotes_and_is_quoted_itself() {
        assert_eq!(shell_word("it's"), r"'it'\''s'");
        assert_eq!(shell_word("'"), r"''\'''");
        assert_eq!(shell_word("''"), r"''\'''\'''");
        assert_eq!(shell_word("a b'"), r"'a b'\'''");
    }

    #[test]
    fn a_word_that_starts_with_a_hyphen_is_not_changed_by_quoting() {
        // Quoting does not hide an option from a command: the argv is what
        // it is, and the quotes only keep the shell from splitting it.
        assert_eq!(shell_word("-x"), "-x");
        assert_eq!(shell_word("--name=a b"), "'--name=a b'");
    }

    #[test]
    fn a_quoted_line_gives_the_shell_the_same_words_back() {
        let words = ["a b", "it's", "$x", "", "x\ny", "back\\slash", "plain", "'"];
        let line: Vec<String> = words.iter().map(|word| shell_word(word)).collect();
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("for word in {}; do printf '%s\\0' \"$word\"; done", line.join(" ")))
            .output()
            .unwrap();
        let got: Vec<&[u8]> = output.stdout.split(|&b| b == 0).collect();
        let got: Vec<String> = got[..got.len() - 1]
            .iter()
            .map(|word| String::from_utf8_lossy(word).into_owned())
            .collect();
        assert_eq!(got, words);
    }

    #[test]
    fn a_change_shows_as_an_administrator_s_command_and_a_read_as_the_person_s() {
        let change = show("service.restart", json!({ "name": "nginx" }));
        assert_eq!(change["command"], "elevate service nginx restart");
        let read = show("service.explain", json!({ "name": "nginx" }));
        assert_eq!(read["command"], "service nginx explain --json");
        let list = show("service.list", json!({}));
        assert_eq!(list["command"], "service list --json");
    }

    #[test]
    fn a_read_is_a_get_with_the_token_and_the_filled_in_address() {
        let read = show("service.explain", json!({ "name": "nginx" }));
        assert_eq!(
            read["api"],
            r#"curl -sS -H "Authorization: Bearer $DUKOVO_TOKEN" https://nas.example:9090/api/1/services/nginx/explain"#
        );
        let list = show("service.list", json!({}));
        assert_eq!(
            list["api"],
            r#"curl -sS -H "Authorization: Bearer $DUKOVO_TOKEN" https://nas.example:9090/api/1/services"#
        );
    }

    #[test]
    fn a_read_s_arguments_the_address_does_not_hold_go_in_its_query() {
        let action = lookup(crate::admind::actions::tests::LAB, "lab.log").unwrap();
        let args = json!({ "lines": 20, "service": "web one", "kernel": false });
        let read = shown(&action.prepare(args).unwrap(), ORIGIN);
        assert_eq!(
            read["api"],
            r#"curl -sS -H "Authorization: Bearer $DUKOVO_TOKEN" 'https://nas.example:9090/api/1/lab/log?lines=20&service=web%20one'"#
        );
        let bare = shown(&action.prepare(json!({})).unwrap(), ORIGIN);
        assert_eq!(
            bare["api"],
            r#"curl -sS -H "Authorization: Bearer $DUKOVO_TOKEN" https://nas.example:9090/api/1/lab/log"#
        );
    }

    #[test]
    fn a_change_is_a_post_with_a_body_of_what_the_address_does_not_hold() {
        let change = show("service.restart", json!({ "name": "nginx" }));
        assert_eq!(
            change["api"],
            r#"curl -sS -X POST -H "Authorization: Bearer $DUKOVO_TOKEN" -H "Content-Type: application/json" -d '{}' https://nas.example:9090/api/1/services/nginx/restart"#
        );
    }

    #[test]
    fn what_the_address_does_not_hold_goes_in_the_body_quoted_as_a_word() {
        let template = "/api/1/lab/{name}/restart";
        let (path, rest) =
            address(template, &json!({ "name": "nginx", "wait": 5, "note": "it's" }));
        assert_eq!(path, "/api/1/lab/nginx/restart");
        assert_eq!(Value::Object(rest).to_string(), r#"{"note":"it's","wait":5}"#);
    }

    #[test]
    fn an_address_part_is_percent_encoded_but_for_unreserved_characters() {
        assert_eq!(percent_encode("nginx-1.2_x~"), "nginx-1.2_x~");
        assert_eq!(percent_encode("a b/c"), "a%20b%2Fc");
        assert_eq!(percent_encode("caf\u{e9}"), "caf%C3%A9");
        assert_eq!(percent_encode("100%"), "100%25");
        assert_eq!(percent_encode("a?b#c"), "a%3Fb%23c");
        assert_eq!(percent_encode(""), "");
        let (path, _) = address("/api/1/x/{name}", &json!({ "name": "a b" }));
        assert_eq!(path, "/api/1/x/a%20b");
        let (path, _) = address("/api/1/x/{port}", &json!({ "port": 8080 }));
        assert_eq!(path, "/api/1/x/8080");
    }

    #[test]
    fn an_origin_with_characters_a_shell_reads_is_quoted() {
        let action = lookup(REGISTRY, "service.list").unwrap();
        let prepared = action.prepare(json!({})).unwrap();
        assert!(
            curl(&prepared, "https://[::1]:9090").ends_with(" 'https://[::1]:9090/api/1/services'")
        );
    }

    #[test]
    fn the_task_is_one_mapping_in_a_list_with_its_keys_in_order() {
        let change = show("certificate.renew", json!({ "name": "this-machine" }));
        assert_eq!(
            change["ansible"],
            "- name: \"Renew a certificate: this-machine\"\n  ansible.builtin.command:\n    argv:\n      - \"cert\"\n      - \"renew\"\n      - \"this-machine\"\n  become: true\n"
        );
        let read = show("service.explain", json!({ "name": "nginx" }));
        assert_eq!(
            read["ansible"],
            "- name: \"Explain a service: nginx\"\n  ansible.builtin.command:\n    argv:\n      - \"service\"\n      - \"nginx\"\n      - \"explain\"\n      - \"--json\"\n  register: answer\n  changed_when: false\n  failed_when: \"answer.rc not in [0, 3]\"\n"
        );
    }

    #[test]
    fn a_restart_and_the_search_are_followed_by_a_wait_for_the_machine_and_a_shut_down_is_not() {
        let wait = "- name: \"Wait for the machine to answer again\"\n  \
                    ansible.builtin.wait_for_connection:\n    delay: 120\n    timeout: 1800\n";
        let restart = show("power.restart", json!({}));
        assert_eq!(
            restart["ansible"],
            format!(
                "- name: \"Restart this machine\"\n  ansible.builtin.command:\n    argv:\n      - \
                 \"shutdown\"\n      - \"-r\"\n      - \"+1\"\n      - \"--no-wall\"\n  become: \
                 true\n{wait}"
            )
        );
        for id in ["updates.bisect", "updates.bisect-confirm"] {
            let search = show(id, json!({}));
            assert!(search["ansible"].as_str().unwrap().ends_with(wait), "{id}: {search}");
        }
        let off = show("power.shutdown", json!({}));
        assert!(!off["ansible"].as_str().unwrap().contains("wait_for_connection"), "{off}");
        // Every action that restarts the machine, but the shut down, waits.
        for action in REGISTRY.iter().filter(|action| action.restarts().is_some()) {
            let good = action.example_values().0.into_iter().next().unwrap();
            let task = show(action.id(), good)["ansible"].as_str().unwrap().to_string();
            assert_eq!(
                task.contains("wait_for_connection"),
                action.id() != "power.shutdown",
                "{task}"
            );
        }
    }

    #[test]
    fn a_service_change_is_a_task_of_the_collection_s_module_with_no_changed_when() {
        let restart = show("service.restart", json!({ "name": "nginx" }));
        assert_eq!(
            restart["ansible"],
            "- name: \"Restart a service: nginx\"\n  dukovo.system.service:\n    name: \"nginx\"\n    state: \"restarted\"\n  become: true\n"
        );
        // The parameters are in the order of their names.
        let enable = show("service.enable", json!({ "name": "nginx" }));
        assert_eq!(
            enable["ansible"],
            "- name: \"Enable a service: nginx\"\n  dukovo.system.service:\n    enabled: true\n    name: \"nginx\"\n    state: \"started\"\n  become: true\n"
        );
    }

    #[test]
    fn a_string_is_written_as_the_json_string_yaml_reads_back_the_same() {
        let task = Ansible::command(
            "Say \"hi\": a\\b\nc\td",
            &["echo".into(), "caf\u{e9} 'x'".into(), String::new(), "- y".into()],
            false,
            true,
        );
        assert_eq!(
            yaml(&task),
            "- name: \"Say \\\"hi\\\": a\\\\b\\nc\\td\"\n  ansible.builtin.command:\n    argv:\n      - \"echo\"\n      - \"caf\u{e9} 'x'\"\n      - \"\"\n      - \"- y\"\n"
        );
    }

    #[test]
    fn a_module_s_parameters_are_scalars_lists_and_mappings() {
        let mut params = Map::new();
        // In the order of their names, which is the order of a sorted map
        // and of one that keeps the order it was given.
        params.insert("empty".into(), json!([]));
        params.insert("enabled".into(), json!(true));
        params.insert("limits".into(), json!({ "memory": "64M", "nested": [[1], { "a": 2 }] }));
        params.insert("name".into(), json!("nginx"));
        params.insert("none".into(), json!(null));
        params.insert("odd key".into(), json!("x"));
        params.insert("ports".into(), json!([80, 443]));
        params.insert("wait".into(), json!(30));
        let task = Ansible {
            name: "Do".into(),
            module: "dukovo.system.service",
            params,
            become_root: true,
            changed_when: Some("result.rc != 0".into()),
            register: Some("result"),
            failed_when: Some("result.rc not in [0, 3]".into()),
            no_log: true,
            when: None,
            comes_back: false,
        };
        assert_eq!(
            yaml(&task),
            "- name: \"Do\"\n  dukovo.system.service:\n    empty: []\n    enabled: true\n    limits:\n      memory: \"64M\"\n      nested:\n        - [1]\n        - {\"a\":2}\n    name: \"nginx\"\n    none: null\n    \"odd key\": \"x\"\n    ports:\n      - 80\n      - 443\n    wait: 30\n  become: true\n  no_log: true\n  register: result\n  changed_when: \"result.rc != 0\"\n  failed_when: \"result.rc not in [0, 3]\"\n"
        );
    }

    #[test]
    fn files_are_the_prepared_action_s() {
        assert_eq!(show("service.restart", json!({ "name": "nginx" }))["files"], json!([]));
    }

    #[test]
    fn an_action_that_is_not_in_the_list_is_adm_125_for_its_field() {
        let failure = lookup(REGISTRY, "service.explode").unwrap_err();
        assert_eq!((failure.kind, failure.field), (Kind::BadValue, Some("action")));
        assert_eq!(
            failure.detail,
            "There is no action service.explode: admin actions lists them. Nothing was done."
        );
        assert!(lookup(REGISTRY, "").is_err());
        assert!(lookup(REGISTRY, "Service.list").is_err());
        assert_eq!(lookup(REGISTRY, "service.list").unwrap().id(), "service.list");
    }
}
