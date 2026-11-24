// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The environment a Linux program starts with: a clean one, as a bundle's
//! program has (`bundles::run::environment`), with Linux's names for the
//! places in its view and none of Dukovo's.
//!
//! `HOME` is the program's data folder and the four XDG folders are beneath
//! it, so a program that keeps its settings and caches by the XDG rules
//! keeps them in its own data and nowhere else. `TZ` is left unset: the
//! zone is `/etc/localtime`'s, which leads to the zone database.

use accounts::User;

/// Where the program finds programs, as Linux's distributions lay them
/// out.
pub const PATH: &str = "/usr/local/bin:/usr/bin:/bin";
/// The language when the caller has none: the one every base has.
const LANG: &str = "C.UTF-8";

/// The environment of a program run for `user`, with `home` its data
/// folder; `caller` is the environment `linux` itself runs in, of which
/// only the terminal type and the language settings go on.
pub fn environment(user: &User, home: &str, caller: &[(String, String)]) -> Vec<String> {
    let mut env = vec![
        format!("HOME={home}"),
        format!("USER={}", user.name),
        format!("LOGNAME={}", user.name),
        format!("PATH={PATH}"),
        format!("XDG_CONFIG_HOME={home}/.config"),
        format!("XDG_DATA_HOME={home}/.local/share"),
        format!("XDG_CACHE_HOME={home}/.cache"),
        format!("XDG_STATE_HOME={home}/.local/state"),
    ];
    let term = caller.iter().find(|(name, _)| name == "TERM").map_or("", |(_, v)| v.as_str());
    env.push(format!("TERM={}", crate::signin::terminal_type(term)));
    let language: Vec<String> = caller
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .filter(|entry| crate::elevate::language_setting(entry))
        .collect();
    if !language.iter().any(|entry| entry.starts_with("LANG=")) {
        env.push(format!("LANG={LANG}"));
    }
    env.extend(language);
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alice() -> User {
        User {
            name: "alice".into(),
            uid: 1000,
            gid: 1000,
            full_name: "Alice Example".into(),
            home: "/users/alice".into(),
            shell: "/system/commands/nukesh".into(),
        }
    }

    fn caller(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect()
    }

    #[test]
    fn the_environment_is_clean_and_the_data_folder_is_home() {
        let env = environment(&alice(), "/users/alice/app-data/linux/hello", &caller(&[]));
        assert_eq!(
            env,
            [
                "HOME=/users/alice/app-data/linux/hello",
                "USER=alice",
                "LOGNAME=alice",
                "PATH=/usr/local/bin:/usr/bin:/bin",
                "XDG_CONFIG_HOME=/users/alice/app-data/linux/hello/.config",
                "XDG_DATA_HOME=/users/alice/app-data/linux/hello/.local/share",
                "XDG_CACHE_HOME=/users/alice/app-data/linux/hello/.cache",
                "XDG_STATE_HOME=/users/alice/app-data/linux/hello/.local/state",
                "TERM=dumb",
                "LANG=C.UTF-8",
            ]
        );
    }

    #[test]
    fn only_the_terminal_type_and_the_language_go_on() {
        let env = environment(
            &alice(),
            "/h",
            &caller(&[
                ("TERM", "xterm-256color"),
                ("LANG", "de_DE.UTF-8"),
                ("LC_TIME", "en_GB.UTF-8"),
                ("TZ", "Europe/Berlin"),
                ("SSH_AUTH_SOCK", "/x"),
                ("LC_ALL", "/etc/passwd"),
                ("PATH", "/system/commands"),
                ("HOME", "/users/alice"),
            ]),
        );
        assert!(env.contains(&"TERM=xterm-256color".to_string()));
        assert!(env.contains(&"LANG=de_DE.UTF-8".to_string()));
        assert!(env.contains(&"LC_TIME=en_GB.UTF-8".to_string()));
        // The default is only for a caller with no LANG.
        assert!(!env.contains(&"LANG=C.UTF-8".to_string()));
        for left_out in ["TZ=", "SSH_AUTH_SOCK=", "LC_ALL=", "PATH=/system"] {
            assert!(!env.iter().any(|entry| entry.starts_with(left_out)), "{left_out}");
        }
        assert_eq!(env.iter().filter(|entry| entry.starts_with("HOME=")).count(), 1);
        assert!(env.contains(&"HOME=/h".to_string()));
    }

    #[test]
    fn a_terminal_type_that_is_no_name_is_dumb() {
        let env = environment(&alice(), "/h", &caller(&[("TERM", "x y;z")]));
        assert!(env.contains(&"TERM=dumb".to_string()));
    }

    #[test]
    fn a_caller_with_other_language_settings_still_gets_the_default_lang() {
        let env = environment(&alice(), "/h", &caller(&[("LC_MESSAGES", "fr_FR.UTF-8")]));
        assert!(env.contains(&"LANG=C.UTF-8".to_string()));
        assert!(env.contains(&"LC_MESSAGES=fr_FR.UTF-8".to_string()));
    }
}
