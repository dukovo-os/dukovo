// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What the receivers and alertd's settings agree on: the ports, the
//! account the mail server takes, and the text of alerts.toml that
//! names them. They are written once, here, so that the boot test never
//! types a secret or a room's ID at a shell.

/// Where the mail server takes submissions, with STARTTLS.
pub const SMTP_PORT: u16 = 587;

/// Where the webhook receiver speaks HTTPS.
pub const HTTPS_PORT: u16 = 8443;

/// Where the receiver speaks plain HTTP, as alertd's `json` webhook may
/// to a private address.
pub const HTTP_PORT: u16 = 8080;

/// The account the mail server takes.
pub const USER: &str = "alerts@example.test";
pub const PASSWORD: &str = "tea party";

/// The folder the receivers write what they get in.
pub const FOLDER: &str = "/shared/monitoring";

/// The paths alerts.toml gives each kind of webhook. Matrix's are below
/// its homeserver's address, which is the receiver's.
pub const NTFY: &str = "/ntfy-topic";
pub const SLACK: &str = "/slack-hook";
pub const DISCORD: &str = "/discord-hook";
pub const JSON: &str = "/json-hook";
pub const MATRIX: &str = "/_matrix/";

/// The path that is answered 403, as a service that refuses a token or a
/// room does.
pub const DENIED: &str = "/denied";

/// The kinds of webhook, in the order alerts.toml names them.
pub const KINDS: [&str; 5] = ["ntfy", "slack", "discord", "matrix", "json"];

/// Which kind of webhook a request to `path` is for, or `denied` for the
/// path that is refused, or `other`.
pub fn kind_of(path: &str) -> &'static str {
    let starts = |prefix: &str| path.starts_with(prefix);
    if starts(NTFY) {
        "ntfy"
    } else if starts(SLACK) {
        "slack"
    } else if starts(DISCORD) {
        "discord"
    } else if starts(MATRIX) {
        "matrix"
    } else if starts(JSON) {
        "json"
    } else if starts(DENIED) {
        "denied"
    } else {
        "other"
    }
}

/// The test authority's certificate, as alertd's `ca` setting names it.
pub const CA_FILE: &str = "/system/settings/ssl/test-ca.pem";

/// The text of alerts.toml for the test: the email and every kind of
/// webhook, all to the receivers. With `broken`, the mail account's
/// password is wrong and the Discord address is one the receiver refuses,
/// so that `alerts test` shows each service's own words.
pub fn alerts_toml(broken: bool) -> String {
    let password = if broken { "coffee" } else { PASSWORD };
    let discord = if broken { DENIED } else { DISCORD };
    format!(
        "[email]\n\
         to = [\"ops@example.test\"]\n\
         from = \"alerts@example.test\"\n\
         server = \"127.0.0.1\"\n\
         port = {SMTP_PORT}\n\
         user = \"{USER}\"\n\
         password = \"{password}\"\n\
         ca = \"{CA_FILE}\"\n\
         \n\
         [[webhook]]\n\
         kind = \"ntfy\"\n\
         url = \"https://127.0.0.1:{HTTPS_PORT}{NTFY}\"\n\
         \n\
         [[webhook]]\n\
         kind = \"slack\"\n\
         url = \"https://127.0.0.1:{HTTPS_PORT}{SLACK}\"\n\
         \n\
         [[webhook]]\n\
         kind = \"discord\"\n\
         url = \"https://127.0.0.1:{HTTPS_PORT}{discord}\"\n\
         \n\
         [[webhook]]\n\
         kind = \"matrix\"\n\
         url = \"https://127.0.0.1:{HTTPS_PORT}\"\n\
         room = \"!room:example.test\"\n\
         token = \"matrix-token\"\n\
         \n\
         [[webhook]]\n\
         kind = \"json\"\n\
         url = \"http://127.0.0.1:{HTTP_PORT}{JSON}\"\n\
         token = \"json-token\"\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_settings_name_the_email_and_each_kind_of_webhook() {
        let text = alerts_toml(false);
        for kind in ["ntfy", "slack", "discord", "matrix", "json"] {
            assert!(text.contains(&format!("kind = \"{kind}\"")), "{kind}");
        }
        assert!(text.contains("[email]") && text.contains(PASSWORD));
        assert!(!text.contains(DENIED) && !text.contains("coffee"));
    }

    #[test]
    fn the_broken_settings_have_a_wrong_password_and_a_refused_address() {
        let text = alerts_toml(true);
        assert!(text.contains("password = \"coffee\"") && !text.contains(PASSWORD));
        assert!(text.contains(&format!("{HTTPS_PORT}{DENIED}")));
    }

    #[test]
    fn a_request_is_of_the_kind_whose_path_it_is_for() {
        assert_eq!(kind_of("/ntfy-topic"), "ntfy");
        assert_eq!(kind_of("/slack-hook"), "slack");
        assert_eq!(kind_of("/discord-hook"), "discord");
        assert_eq!(
            kind_of("/_matrix/client/v3/rooms/%21room%3Aexample.test/send/m.room.message/7"),
            "matrix"
        );
        assert_eq!(kind_of("/json-hook"), "json");
        assert_eq!(kind_of("/denied"), "denied");
        assert_eq!(kind_of("/elsewhere"), "other");
    }

    #[test]
    fn the_paths_the_settings_name_are_the_paths_that_are_told_apart() {
        let text = alerts_toml(false);
        for path in [NTFY, SLACK, DISCORD, JSON] {
            assert!(text.contains(path), "{path}");
        }
        assert!(KINDS.iter().all(|kind| text.contains(&format!("kind = \"{kind}\""))));
    }
}
