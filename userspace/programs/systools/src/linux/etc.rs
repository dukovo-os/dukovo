// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The text of the files in the program's `/etc` that `linux run` makes:
//! the accounts, the host names, the name service switch and the machine
//! ID, in the formats glibc and musl read, and the time zone `localtime`
//! leads to.
//!
//! Each is a pure function of what Dukovo keeps, so the tests hold the
//! exact text. `private.rs` writes them to the run's private folder, and
//! `view.rs` attaches each at its name in `/etc`. When a program reads the
//! wrong account or host name, look at the file in the private folder
//! first (`/tmp/.linux-PID/etc` while it runs).

use accounts::User;

/// The shell of every account in the view: the base's, as `/bin` leads to
/// its `usr/bin`.
const SHELL: &str = "/bin/sh";
/// `nobody`'s number, and its group's, as Linux's distributions give them.
const NOBODY: u32 = 65534;

/// Where Dukovo keeps the time zone database.
pub const DUKOVO_ZONEINFO: &str = "/system/resources/zoneinfo";
/// Where Linux programs look for the time zone database.
pub const ZONEINFO: &str = "/usr/share/zoneinfo";

/// `passwd(5)`: root, the person, whose home is `home` (the program's data
/// folder), and `nobody`. The person is root's line when they are root.
pub fn passwd(user: &User, home: &str) -> String {
    let mut text = String::new();
    if user.uid != 0 {
        text += &format!("root:x:0:0:root:/root:{SHELL}\n");
    }
    let full_name = user.full_name.replace([':', '\n'], " ");
    let (name, uid, gid) = (&user.name, user.uid, user.gid);
    text += &format!("{name}:x:{uid}:{gid}:{full_name}:{home}:{SHELL}\n");
    text += &format!("nobody:x:{NOBODY}:{NOBODY}:nobody:/nonexistent:/usr/sbin/nologin\n");
    text
}

/// `group(5)`: root's group, the person's, named `name`, and `nogroup`.
pub fn group(user: &User, name: &str) -> String {
    let mut text = String::new();
    if user.gid != 0 {
        text += "root:x:0:\n";
    }
    text += &format!("{name}:x:{}:\n", user.gid);
    text += &format!("nogroup:x:{NOBODY}:\n");
    text
}

/// The host name as `/etc/hostname` and the program's own `hosts` file
/// want it: what the kernel says, trimmed, and `localhost` if it says
/// nothing.
pub fn host_name(kernel: &str) -> String {
    let name = kernel.trim();
    if name.is_empty() { "localhost".into() } else { name.into() }
}

/// `hosts(5)`: the loopback names, and the host name on 127.0.1.1 as
/// Debian's installer writes it.
pub fn hosts(host: &str) -> String {
    let mut text = String::from(
        "127.0.0.1\tlocalhost\n\
         ::1\tlocalhost ip6-localhost ip6-loopback\n",
    );
    if host != "localhost" {
        text += &format!("127.0.1.1\t{host}\n");
    }
    text
}

/// `nsswitch.conf(5)`: accounts from the files above, names from `hosts`,
/// then the name servers `resolv.conf` names.
pub const NSSWITCH: &str = "passwd: files\ngroup: files\nhosts: files dns\n";

/// A machine ID as `machine-id(5)` writes it: 32 lowercase hexadecimal
/// digits and a newline.
pub fn machine_id(random: &[u8; 16]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(33);
    for byte in random {
        text.push(char::from(DIGITS[usize::from(byte >> 4)]));
        text.push(char::from(DIGITS[usize::from(byte & 15)]));
    }
    text.push('\n');
    text
}

/// Whether `text` is a machine ID, as [`machine_id`] writes one.
pub fn is_machine_id(text: &str) -> bool {
    text.strip_suffix('\n').is_some_and(|id| {
        id.len() == 32
            && id.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

/// The zone, such as `Europe/Berlin`, that Dukovo's `localtime` link
/// leads to (`target`, a path beneath [`DUKOVO_ZONEINFO`]); `UTC` when it
/// leads anywhere else.
pub fn zone_of(target: &str) -> &str {
    let zone = target.strip_prefix(DUKOVO_ZONEINFO).and_then(|rest| rest.strip_prefix('/'));
    let plain =
        |zone: &&str| zone.split('/').all(|part| !part.is_empty() && !part.starts_with('.'));
    zone.filter(plain).unwrap_or("UTC")
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

    #[test]
    fn passwd_holds_root_the_person_and_nobody() {
        assert_eq!(
            passwd(&alice(), "/users/alice/app-data/linux/hello"),
            "root:x:0:0:root:/root:/bin/sh\n\
             alice:x:1000:1000:Alice Example:/users/alice/app-data/linux/hello:/bin/sh\n\
             nobody:x:65534:65534:nobody:/nonexistent:/usr/sbin/nologin\n"
        );
    }

    #[test]
    fn root_has_one_line_when_the_person_is_root() {
        let root =
            User { name: "root".into(), uid: 0, gid: 0, full_name: "root".into(), ..alice() };
        assert_eq!(
            passwd(&root, "/root/app-data/linux/hello"),
            "root:x:0:0:root:/root/app-data/linux/hello:/bin/sh\n\
             nobody:x:65534:65534:nobody:/nonexistent:/usr/sbin/nologin\n"
        );
        assert_eq!(group(&root, "root"), "root:x:0:\nnogroup:x:65534:\n");
    }

    #[test]
    fn a_full_name_cannot_start_a_line_of_its_own() {
        let user = User { full_name: "Alice:x:0:0\nroot".into(), ..alice() };
        let text = passwd(&user, "/h");
        assert_eq!(text.lines().count(), 3, "{text}");
        assert!(text.contains("alice:x:1000:1000:Alice x 0 0 root:/h:/bin/sh\n"), "{text}");
    }

    #[test]
    fn group_holds_root_the_persons_group_and_nogroup() {
        assert_eq!(group(&alice(), "users"), "root:x:0:\nusers:x:1000:\nnogroup:x:65534:\n");
    }

    #[test]
    fn hosts_name_the_loopback_and_the_machine() {
        assert_eq!(
            hosts("dukovo-pc"),
            "127.0.0.1\tlocalhost\n\
             ::1\tlocalhost ip6-localhost ip6-loopback\n\
             127.0.1.1\tdukovo-pc\n"
        );
        assert_eq!(
            hosts("localhost"),
            "127.0.0.1\tlocalhost\n::1\tlocalhost ip6-localhost ip6-loopback\n"
        );
    }

    #[test]
    fn the_host_name_is_what_the_kernel_says_or_localhost() {
        assert_eq!(host_name("dukovo-pc\n"), "dukovo-pc");
        assert_eq!(host_name(""), "localhost");
        assert_eq!(host_name("  \n"), "localhost");
    }

    #[test]
    fn the_name_service_reads_files_then_the_name_servers() {
        assert_eq!(NSSWITCH, "passwd: files\ngroup: files\nhosts: files dns\n");
    }

    #[test]
    fn a_machine_id_is_32_hexadecimal_digits() {
        let id = machine_id(&[0x00, 0x01, 0xab, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x10, 0x20]);
        assert_eq!(id, "0001abff000000000000000000001020\n");
        assert!(is_machine_id(&id));
        for not in
            ["", "\n", "0001abff00000000000000000000102\n", "0001ABFF000000000000000000001020\n"]
        {
            assert!(!is_machine_id(not), "{not:?}");
        }
        // No newline, and one digit too many.
        assert!(!is_machine_id("0001abff00000000000000000000102"));
        assert!(!is_machine_id("0001abff000000000000000000001020 \n"));
    }

    #[test]
    fn the_zone_is_what_dukovos_localtime_leads_to() {
        assert_eq!(zone_of("/system/resources/zoneinfo/Europe/Berlin"), "Europe/Berlin");
        assert_eq!(zone_of("/system/resources/zoneinfo/UTC"), "UTC");
        for elsewhere in [
            "/usr/share/zoneinfo/Europe/Berlin",
            "/system/resources/zoneinfo",
            "/system/resources/zoneinfo/",
            "/system/resources/zoneinfo/../../../etc/passwd",
            "/system/resources/zoneinfo/Europe//Berlin",
            "zoneinfo/Europe/Berlin",
        ] {
            assert_eq!(zone_of(elsewhere), "UTC", "{elsewhere}");
        }
    }
}
