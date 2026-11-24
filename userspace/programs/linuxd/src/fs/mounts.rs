// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! `/proc/<n>/mounts` and `mountinfo`: the sandbox's mounts, which `linux
//! run` writes in `/proc/mounts` format to a file whose path is in the
//! environment variable `LINUXD_MOUNTS`. `mounts` is that file; `mountinfo`
//! is made from its lines.
//!
//! The launcher gives no mount an identifier or a device number, and a
//! sandbox has no bind mounts or peer groups, so `mountinfo` numbers the
//! mounts from 1 in the file's order, takes a mount's parent to be the
//! latest earlier mount that covers it, gives each a device number `0:N`
//! and leaves the optional fields (`shared:`, `master:`) out.

use super::text::Page;

/// The environment variable that names the file of mounts.
pub const VARIABLE: &str = "LINUXD_MOUNTS";

/// The mounts of a sandbox whose launcher wrote none: the root alone.
pub const ROOT_ONLY: &str = "rootfs / rootfs rw 0 0\n";

/// One mount of the file: the fields of Linux's `/proc/mounts`.
struct Mount<'a> {
    source: &'a str,
    target: &'a str,
    kind: &'a str,
    options: &'a str,
}

impl<'a> Mount<'a> {
    /// The mount a line of `/proc/mounts` describes, if it has the four
    /// fields that matter.
    fn parse(line: &'a str) -> Option<Mount<'a>> {
        let mut fields = line.split(' ');
        Some(Mount {
            source: fields.next()?,
            target: fields.next()?,
            kind: fields.next()?,
            options: fields.next()?,
        })
    }

    /// Whether this mount's directory is `inner` or a place beneath it.
    fn covers(&self, inner: &str) -> bool {
        self.target == "/"
            || inner == self.target
            || inner.strip_prefix(self.target).is_some_and(|rest| rest.starts_with('/'))
    }
}

/// `/proc/<n>/mountinfo`, from the text of the mounts file.
pub fn mountinfo(mounts: &str) -> String {
    let all: Vec<Mount<'_>> = mounts.lines().filter_map(Mount::parse).collect();
    let mut page = Page::new();
    for (index, mount) in all.iter().enumerate() {
        let id = index + 1;
        // The longest covering directory wins; the later of two at the
        // same place, which lies over the earlier.
        let parent = all[..index]
            .iter()
            .enumerate()
            .filter(|(_, earlier)| earlier.covers(mount.target))
            .max_by_key(|(at, earlier)| (earlier.target.len(), *at))
            .map_or(0, |(at, _)| at + 1);
        let read_only = mount.options.split(',').next() == Some("ro");
        page.line(format_args!(
            "{id} {parent} 0:{id} / {} {} - {} {} {}",
            mount.target,
            mount.options,
            mount.kind,
            mount.source,
            if read_only { "ro" } else { "rw" },
        ));
    }
    page.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOUNTS: &str = "rootfs / rootfs rw 0 0
usr /usr bind ro,nosuid 0 0
proc /proc proc rw,nosuid,nodev,noexec 0 0
tmpfs /tmp tmpfs rw,nosuid,nodev 0 0
tmpfs /tmp/a\\040b tmpfs rw 0 0
sys /sys/fs/cgroup cgroup2 ro 0 0
";

    #[test]
    fn mountinfo_numbers_the_mounts_and_finds_their_parents() {
        let golden = "1 0 0:1 / / rw - rootfs rootfs rw
2 1 0:2 / /usr ro,nosuid - bind usr ro
3 1 0:3 / /proc rw,nosuid,nodev,noexec - proc proc rw
4 1 0:4 / /tmp rw,nosuid,nodev - tmpfs tmpfs rw
5 4 0:5 / /tmp/a\\040b rw - tmpfs tmpfs rw
6 1 0:6 / /sys/fs/cgroup ro - cgroup2 sys ro
";
        assert_eq!(mountinfo(MOUNTS), golden);
    }

    #[test]
    fn a_mount_over_another_has_it_as_its_parent() {
        let text = "a / x rw 0 0\nb /m x rw 0 0\nc /m x rw 0 0\n";
        let lines: Vec<String> = mountinfo(text).lines().map(str::to_string).collect();
        assert!(lines[1].starts_with("2 1 "));
        assert!(lines[2].starts_with("3 2 "));
    }

    #[test]
    fn a_directory_that_only_starts_alike_is_not_a_parent() {
        let text = "a / x rw 0 0\nb /tmp x rw 0 0\nc /tmpfile x rw 0 0\n";
        let lines: Vec<String> = mountinfo(text).lines().map(str::to_string).collect();
        assert!(lines[2].starts_with("3 1 "), "{}", lines[2]);
    }

    #[test]
    fn the_default_is_the_root_alone() {
        assert_eq!(mountinfo(ROOT_ONLY), "1 0 0:1 / / rw - rootfs rootfs rw\n");
    }

    #[test]
    fn a_short_line_is_left_out() {
        assert_eq!(mountinfo("one two\n"), "");
    }
}
