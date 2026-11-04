// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A bundle from no catalog updates from its own feed, a small TUF
//! repository rooted in the key its manifest names (ADR 0390). A second
//! disk holds almanac 1.0.0, whose manifest names its feed at 10.0.2.100,
//! where QEMU's user network runs a small HTTP server of the host's for
//! each connection; the build key signs the files, and `bundle feed`
//! signed the feed with it. Alice installs almanac from its file, and its
//! sheet names the feed. bundle update --check finds 1.1.0 there, and
//! bundle update fetches it, checks it as TUF does and installs it; it
//! runs, and bundle history says it came from its feed. The host then
//! serves the feed as it was signed before, which TUF refuses as older
//! than the one trusted; then a newer one that lists 1.2.0, whose file
//! differs from the one it signs, which is refused and leaves 1.1.0.
//! bundle feed refuses to sign almanac's feed with Alice's developer key,
//! which its manifest does not name, and signs a feed for a bundle of her
//! own. The power button ends it.

use std::path::Path;

use super::super::*;

/// The disk's label.
const LABEL: &str = "FEED";

/// Where Spark mounts it: the second virtio disk's name.
const VOLUME: &str = "/volumes/vdb";

/// What the host serves, from the top of the tree, which QEMU runs in.
const DIR: &str = "out/test/bundle-feed";

/// The test's network: QEMU's user network, with the host's server at
/// 10.0.2.100 port 80.
const NETDEV: &str = "user,id=n0,guestfwd=tcp:10.0.2.100:80-cmd:out/test/bundle-feed/serve";

/// almanac 1.0.0's file, on the disk.
const FILE: &str = "org.example.almanac-1.0.0.dkv";

/// Dukovo's root on a copy of the test image, and almanac's first file.
const DISKS: &[Disk] = &[
    Disk { label: image::ROOT_LABEL, ..btrfs_disk(Bus::Virtio, 512, ROOT, root_checked) },
    feed_files_disk(16 * 1024, LABEL),
];

/// The test.
pub const CASE: Case = Case {
    name: "bundle-feed",
    about: "a bundle from no catalog updates from its own feed, a small TUF repository rooted in its manifest's key, over HTTP (ADR 0390): the sheet names the feed; bundle update --check finds the newer version and bundle update installs it, checked as TUF checks; bundle history says so; an old feed served again and a file that differs from the one the feed signs are refused; bundle feed signs a feed, and refuses a key the manifest does not name",
    firmware: Firmware::Uefi,
    media: Media::Usb,
    // As a normal boot: Spark starts the services, bundled and netd among
    // them.
    cmdline: "lockdep",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Plain,
    cpu_model: None,
    qemu: &["-netdev", NETDEV, "-device", "virtio-net-pci,netdev=n0"],
    script: bundle_feed,
    disks: DISKS,
    second: None,
};

/// Copies the folder `from`, with everything in it, to `to`.
fn copy(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|err| format!("making {}: {err}", to.display()))?;
    let entries =
        std::fs::read_dir(from).map_err(|err| format!("reading {}: {err}", from.display()))?;
    for entry in entries {
        let entry = entry.map_err(|err| format!("reading {}: {err}", from.display()))?;
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)
                .map_err(|err| format!("writing {}: {err}", target.display()))?;
        }
    }
    Ok(())
}

/// Serves the feed `name` of out/test/bundle-feed/feeds in place of the
/// one served.
fn serve(name: &str) -> Result<(), String> {
    let to = Path::new(DIR).join("site/almanac");
    std::fs::remove_dir_all(&to).map_err(|err| format!("removing {}: {err}", to.display()))?;
    copy(&Path::new(DIR).join("feeds").join(name), &to)
}

fn bundle_feed(vm: &mut Vm, case: &Case) -> Result<(), String> {
    owner_signs_in(vm, case)?;
    vm.expect_anywhere("bundled: 0 bundles installed; answering at /system/live/bundled/socket")?;
    vm.expect_anywhere(&format!("spark: mounted the FAT filesystem of vdb at {VOLUME}"))?;
    vm.expect_anywhere("netd: net0 has 10.0.2.15/24")?;
    // From its file: its key is new here, and its sheet names its feed.
    answer_prompt(vm, "$", &format!("clear; cd {VOLUME}; bundle install --yes {FILE}; cd"))?;
    vm.expect_screen(&[
        &format!("It updates from its own feed at {FEED_ADDRESS}"),
        "installed org.example.almanac 1.0.0, for you",
    ])?;
    answer_prompt(vm, "$", "clear; bundle update --check")?;
    vm.expect_screen(&[
        "INSTALLED  NEWEST  CATALOG",
        "org.example.almanac  1.0.0      1.1.0   its own feed",
        "1 to update, 0 up to date",
    ])?;
    answer_prompt(vm, "$", "clear; bundle update --yes; almanac")?;
    vm.expect_screen(&[
        "installed org.example.almanac 1.1.0 in place of 1.0.0, for you",
        "1 updated, 0 not, 0 up to date",
        "almanac 1.1.0 ran",
    ])?;
    answer_prompt(vm, "$", "clear; bundle history org.example.almanac")?;
    vm.expect_screen(&[
        "org.example.almanac  installed 1.0.0, from a file",
        "org.example.almanac  updated 1.0.0 to 1.1.0, from its feed",
    ])?;
    // The feed as it was signed before, served again in place of the one
    // read.
    serve("old")?;
    answer_prompt(vm, "$", "clear; bundle update --check")?;
    vm.expect_screen(&["its feed: timestamp version 1 is older than", "0 to update, 0 up to date"])?;
    // A newer one, whose 1.2.0 is not the file it signs.
    serve("changed")?;
    answer_prompt(vm, "$", "clear; bundle update --yes; almanac")?;
    vm.expect_screen(&[
        "org.example.almanac 1.2.0: sha256 is",
        "0 updated, 0 not, 0 up to date",
        "almanac 1.1.0 ran",
    ])?;
    vm.expect_screen_without("1.2.0 ran")?;
    // A publisher's side: the key almanac names is not Alice's.
    answer_prompt(
        vm,
        "$",
        &format!("clear; cd {VOLUME}; bundle feed /tmp/feed {FILE} 2>&1 | tr ',' '\\n'; cd"),
    )?;
    vm.expect_screen(&["its manifest names another key"])?;
    // Her own bundle's feed, which her key signs.
    answer_prompt(
        vm,
        "$",
        "clear; cd; bundle new org.example.jot jot > /dev/null; echo echo jot > jot/bin/jot; \
         chmod 755 jot/bin/jot; bundle pack jot -o jot.dkv > /dev/null; bundle feed jots \
         jot.dkv",
    )?;
    vm.expect_screen(&[
        "signed the feed in jots/site: 1 file, 1 new, good until",
        "note: no file's manifest names this feed",
    ])?;
    vm.press_power_button()?;
    vm.expect("spark: every service has stopped")?;
    vm.expect("Power: `spark` asked to power off the machine; syncing the filesystems")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
