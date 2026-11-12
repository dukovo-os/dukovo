// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! An installed system's start menu in Dukovo's colours (ADR 0505). The
//! first start, seen through, keeps the menu up for a minute and a half
//! when it shows, as the menu's timeout on the EFI system partition is
//! made 90 seconds, and gets a health check that never ends. The next start is
//! cut short by a power cut, so the start after it shows the menu (ADR
//! 0501). The screen draws it with the slot's theme, beside its GRUB: the
//! slate background, the box's blue-grey line and the chosen entry on
//! tan. The serial port gets GRUB's plain menu, with Troubleshooting on
//! it, and Enter starts Dukovo, long before the menu's countdown would.
//! GRUB says "Booting" only for an entry it starts by itself, so the
//! test tells Enter's start from the countdown's by the time it takes.

use super::super::*;
use super::update_stage::{power_off, restart, run, started};

/// The test.
pub const CASE: Case = Case {
    name: "start-menu",
    about: "an installed system's start menu, shown after a start cut short, is drawn on the screen in Dukovo's colours from the slot's theme, and plain on the serial port; Enter starts Dukovo (ADR 0505)",
    firmware: Firmware::Uefi,
    media: Media::Disk,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: true,
    image: TestImage::Installed,
    cpu_model: None,
    qemu: &[],
    script: start_menu,
    disks: &[],
    second: None,
};

/// A health check that never ends, so that no start is seen through.
const NEVER_HEALTHY: &str =
    "[[check]]\\nname = \"wait\"\\ncommand = [\"sleep\", \"600\"]\\nrequired = true\\ntimeout = 600\\n";

/// The test disk's EFI system partition, its second.
const ESP: &str = "sda2";

/// How long the menu may take to be drawn once the serial port has it.
const DRAWN_SECONDS: u64 = 15;

/// The menu's timeout on the EFI system partition, in seconds.
const MENU_SECONDS: u64 = 90;

/// How soon Dukovo must have started after Enter: well inside what is
/// left of the menu's countdown once the menu is drawn, so that a start
/// this soon is Enter's.
const STARTED_SECONDS: u64 = 40;

/// Shows the menu, and looks at it.
fn start_menu(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    started(vm, 'a')?;
    vm.expect_anywhere(&format!(
        "sysupdate: this start of slot A (Dukovo {VERSION}) is seen through"
    ))?;
    run(
        vm,
        &format!(
            "printf '{NEVER_HEALTHY}' > /system/settings/sysupdate/health.toml && mkdir -p \
             /tmp/esp && mount -t fat {ESP} /tmp/esp && sed -i 's/^set dukovo_timeout=0$/set \
             dukovo_timeout={MENU_SECONDS}/' /tmp/esp/boot/grub/grub.cfg && grep -q '^set \
             dukovo_timeout={MENU_SECONDS}$' \
             /tmp/esp/boot/grub/grub.cfg && umount /tmp/esp && sync && echo \"menu: $((6 * 7))\""
        ),
        &["menu: 42"],
    )?;

    // A start cut short once it reaches its services, before its health
    // check sees it through.
    restart(vm)?;
    vm.expect("spark: started slot a, @system.a")?;
    vm.expect("spark: started logd (")?;
    vm.monitor("system_reset")?;

    // The serial port's menu is GRUB's own.
    vm.expect("Dukovo Server")?;
    vm.expect("Troubleshooting")?;
    vm.expect("The highlighted entry will be executed automatically in")?;

    // The screen's is the theme's: its background from corner to corner,
    // the box's line, and the chosen entry's tan.
    let background = crate::grub_theme::BACKGROUND;
    let deadline = Instant::now() + Duration::from_secs(DRAWN_SECONDS);
    let image = loop {
        let image = vm.screenshot()?;
        if image.pixel(0, 0) == background || Instant::now() > deadline {
            break image;
        }
        std::thread::sleep(Duration::from_millis(250));
    };
    let corners = [
        (0, 0),
        (image.width - 1, 0),
        (0, image.height - 1),
        (image.width - 1, image.height - 1),
    ];
    for (x, y) in corners {
        if image.pixel(x, y) != background {
            return Err(format!(
                "the screen's corner at ({x}, {y}) is {:02x?}, not the theme's background",
                image.pixel(x, y)
            ));
        }
    }
    let count = |rgb: [u8; 3]| {
        (0..image.height)
            .flat_map(|y| (0..image.width).map(move |x| (x, y)))
            .filter(|&(x, y)| image.pixel(x, y) == rgb)
            .count()
    };
    // The chosen entry's bar is the box's width less its border, a row
    // of the font high; the box's line runs round it, two pixels thick.
    let chosen = count(crate::grub_theme::TAN);
    let line = count(crate::grub_theme::BLUE_GREY);
    if chosen < 500 * 20 || line < 2 * 1000 {
        return Err(format!(
            "the menu shows {chosen} pixels of the chosen entry's tan and {line} of the box's \
             blue grey: it is not drawn with the theme"
        ));
    }

    // Enter starts the default entry, Dukovo, at once: the countdown
    // would take more than another half minute.
    let pressed = Instant::now();
    vm.press(&["ret"])?;
    started(vm, 'a')?;
    let took = pressed.elapsed().as_secs();
    if took > STARTED_SECONDS {
        return Err(format!(
            "Dukovo started {took} s after Enter, as late as the menu's countdown of \
             {MENU_SECONDS} s would start it: Enter did not choose the entry"
        ));
    }
    power_off(vm)
}
