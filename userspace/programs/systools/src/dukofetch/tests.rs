// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use super::*;

/// The Dell of the mock, with a release's numbers.
fn dell() -> Facts {
    Facts {
        user: "chase".into(),
        host: "dell".into(),
        os: Some(("Dukovo Server".into(), "0.12.0".into())),
        machine: "x86_64".into(),
        kernel: Some(("Radix".into(), "0.12.0".into())),
        uptime: Some(3 * 86_400 + 4 * 3600 + 12 * 60 + 7),
        bundles: Some(14),
        shell: Some(Shell {
            path: "/system/commands/nukesh".into(),
            name: "Nukesh".into(),
            version: Some("0.12.0".into()),
        }),
        terminal: Some(Terminal { program: Some("consoled".into()), device: "pts/0".into() }),
        cpu: Some(Cpu { name: "Intel Core i7-8550U".into(), count: Some(8), mhz: Some(4000) }),
        gpus: vec!["Intel UHD Graphics 620".into()],
        memory: Some((2_254_857_830, 16_642_998_272)),
        disk: Some(Disk {
            used: 38 * GIB,
            total: 238 * GIB,
            percent: 16,
            kind: Some("btrfs".into()),
        }),
        addresses: vec![("net0".into(), Cidr::parse("192.168.1.42/24").unwrap())],
    }
}

/// What a terminal shows of `text`: without its escape sequences.
fn shown(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for c in chars.by_ref() {
                if c == 'm' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[test]
fn reads_the_kernel_s_version() {
    let parsed = parse_version("Radix 0.12.0 (Dukovo Server 0.12.0)\n").unwrap();
    assert_eq!(parsed.0, ("Radix".into(), "0.12.0".into()));
    assert_eq!(parsed.1, ("Dukovo Server".into(), "0.12.0".into()));
    assert_eq!(parse_version("Radix 0.12.0"), None);
}

#[test]
fn names_processors_as_neofetch_does() {
    assert_eq!(cpu_name("Intel(R) Core(TM) i7-8550U CPU @ 1.80GHz"), "Intel Core i7-8550U");
    assert_eq!(cpu_name("AMD Ryzen 7 5800X 8-Core Processor"), "AMD Ryzen 7 5800X");
    assert_eq!(
        cpu_name("AMD Ryzen AI 9 HX 370 w/ Radeon 890M"),
        "AMD Ryzen AI 9 HX 370 w/ Radeon 890M"
    );
    assert_eq!(cpu_name("QEMU Virtual CPU version 2.5+"), "QEMU Virtual CPU version 2.5+");
    assert_eq!(cpu_name("Intel(R) Xeon(R) CPU E3-1270 V2 @ 3.50GHz"), "Intel Xeon CPU E3-1270 V2");
    assert_eq!(named_mhz("Intel(R) Core(TM) i7-8550U CPU @ 1.80GHz"), Some(1800));
    assert_eq!(named_mhz("AMD Ryzen 7 5800X 8-Core Processor"), None);
}

#[test]
fn names_display_controllers_as_neofetch_does() {
    let intel = gpu_name(Some("Intel Corporation"), Some("UHD Graphics 620"), (0x8086, 0x5917));
    assert_eq!(intel, "Intel UHD Graphics 620");
    let amd = gpu_name(
        Some("Advanced Micro Devices, Inc. [AMD/ATI]"),
        Some("Navi 23 [Radeon RX 6600/6600 XT/6600M]"),
        (0x1002, 0x73ff),
    );
    assert_eq!(amd, "AMD Radeon RX 6600/6600 XT/6600M");
    let nvidia = gpu_name(Some("NVIDIA Corporation"), Some("GA106 [GeForce RTX 3060]"), (0, 0));
    assert_eq!(nvidia, "NVIDIA GeForce RTX 3060");
    assert_eq!(gpu_name(Some("Red Hat, Inc."), None, (0x1af4, 0x1050)), "Red Hat Device 1050");
    assert_eq!(gpu_name(None, None, (0x1234, 0x1111)), "Device 1234:1111");
}

#[test]
fn says_uptimes_and_sizes_as_neofetch_does() {
    assert_eq!(uptime(3 * 86_400 + 4 * 3600 + 12 * 60 + 59), "3 days, 4 hours, 12 mins");
    assert_eq!(uptime(86_400 + 3600 + 60), "1 day, 1 hour, 1 min");
    assert_eq!(uptime(2 * 86_400 + 30), "2 days");
    assert_eq!(uptime(59), "0 mins");
    assert_eq!(memory(2_254_857_830), "2.1 GiB");
    assert_eq!(memory(16_642_998_272), "15.5 GiB");
    assert_eq!(memory(500 * MIB), "500 MiB");
    assert_eq!(room(38 * GIB + GIB / 3), "38 GiB");
    assert_eq!(room(GIB * 3 / 2), "1.5 GiB");
    assert_eq!(room(300 * MIB), "300 MiB");
    assert_eq!(room(TIB * 18 / 10), "1.8 TiB");
}

#[test]
fn every_line_of_the_dell() {
    let every: Vec<String> =
        lines(&dell(), false).iter().map(|(label, value)| format!("{label}: {value}")).collect();
    assert_eq!(
        every,
        [
            "OS: Dukovo Server 0.12.0 (x86-64)",
            "Kernel: Radix 0.12.0",
            "Uptime: 3 days, 4 hours, 12 mins",
            "Apps: 14 bundles",
            "Shell: Nukesh 0.12.0",
            "Terminal: consoled (pts/0)",
            "CPU: Intel Core i7-8550U (8) @ 4.00 GHz",
            "GPU: Intel UHD Graphics 620",
            "Memory: 2.1 GiB / 15.5 GiB (14%)",
            "Disk (/): 38 GiB / 238 GiB (16%) btrfs",
            "Address: 192.168.1.42/24 (net0)",
        ]
    );
    let narrow: Vec<&str> = lines(&dell(), true).iter().map(|(label, _)| *label).collect();
    assert_eq!(narrow, ["OS", "Kernel", "Uptime", "Apps", "Shell", "CPU", "Memory", "Disk (/)"]);
}

#[test]
fn leaves_out_what_has_no_source() {
    let facts = Facts { user: "root".into(), host: "box".into(), ..Facts::default() };
    assert!(lines(&facts, false).is_empty());
    assert_eq!(plain_text(&facts, None), "root@box\n");
    let cpu = Cpu { name: "QEMU Virtual CPU version 2.5+".into(), count: None, mhz: Some(2999) };
    let facts = Facts { cpu: Some(cpu), bundles: Some(1), ..facts };
    let some = lines(&facts, false);
    assert_eq!(some[0], ("Apps", "1 bundle".into()));
    assert_eq!(some[1], ("CPU", "QEMU Virtual CPU version 2.5+ @ 3.00 GHz".into()));
}

#[test]
fn plain_text_is_the_lines() {
    let text = plain_text(&dell(), None);
    assert!(
        text.starts_with("chase@dell\nOS: Dukovo Server 0.12.0 (x86-64)\nKernel: Radix 0.12.0\n")
    );
    assert!(!text.contains('\x1b'));
    let coloured = plain_text(&dell(), Some(Mode::True));
    assert!(
        coloured.starts_with("\x1b[1;38;2;200;135;58mchase\x1b[0m\x1b[38;2;68;89;107m@\x1b[0m")
    );
    assert_eq!(shown(&coloured), text);
}

#[test]
fn the_pictures_are_whole() {
    for (picture, rows, width) in [(&duke::DUKE_16[..], 32, 24), (&duke::DUKE_10[..], 20, 15)] {
        assert_eq!(picture.len(), rows);
        for row in picture {
            assert_eq!(row.len(), width, "{row:?}");
            for letter in row.bytes().filter(|&b| b != b' ') {
                assert!(pixel(letter).is_some(), "{} is no colour", letter as char);
            }
        }
    }
    for &(letter, _) in duke::COLOURS {
        let used =
            duke::DUKE_16.iter().chain(&duke::DUKE_10).any(|row| row.bytes().any(|b| b == letter));
        assert!(used, "{} is not drawn", letter as char);
    }
}

#[test]
fn draws_two_pixels_a_cell() {
    // Steel over tan, steel alone, tan over nothing, a gap, then nothing
    // over steel and steel over steel.
    let picture = ["dd x  d", "x    dd"];
    let line = &draw(&picture, Mode::True)[0];
    let (steel, tan) = ("\x1b[38;2;68;89;107m", "\x1b[38;2;202;136;57m");
    let on_tan = "\x1b[48;2;202;136;57m";
    let expected = format!("{steel}{on_tan}▀\x1b[0m{steel}▀ {tan}▀ {steel}▄█\x1b[0m");
    assert_eq!(*line, expected);
    assert_eq!(shown(line), "▀▀ ▀ ▄█");
    // In 256 colours, near shades are one, and so a full block.
    let near = ["c", "d"];
    assert_eq!(draw(&near, Mode::True)[0], "\x1b[38;2;60;86;110m\x1b[48;2;68;89;107m▀\x1b[0m");
    assert_eq!(draw(&near, Mode::Indexed)[0], "\x1b[38;5;240m█\x1b[0m");
}

#[test]
fn picks_the_nearest_of_256_colours() {
    assert_eq!(nearest_256(0x000000), 16);
    assert_eq!(nearest_256(0xffffff), 231);
    assert_eq!(nearest_256(0xff0000), 196);
    assert_eq!(nearest_256(0x808080), 244);
    assert_eq!(nearest_256(0x080808), 232);
    // Duke's tan, and his steel, which is nearest a grey.
    assert_eq!(nearest_256(TAN), 173);
    assert_eq!(nearest_256(STEEL), 240);
}

#[test]
fn draws_duke_beside_the_lines() {
    for columns in [100, 80, 79, 48, 40] {
        for mode in [Mode::True, Mode::Indexed] {
            let text = beside_duke(&dell(), mode, columns);
            let rows_drawn: Vec<&str> = text.lines().collect();
            // Duke's 16 rows, or the narrow lines, which run two rows past
            // the smaller Duke's 10; without the palette at 40 columns.
            let rows = match columns {
                80.. => 16,
                41.. => 12,
                _ => 10,
            };
            assert_eq!(rows_drawn.len(), rows, "{columns}");
            for line in &rows_drawn {
                let visible = shown(line);
                assert!(util::width(&visible) <= columns, "{columns} columns: {visible:?}");
            }
            assert_eq!(text.contains("38;2;"), mode == Mode::True);
            assert!(shown(rows_drawn[0]).ends_with("   chase@dell"), "{columns}");
        }
    }
    let wide = shown(&beside_duke(&dell(), Mode::True, 100));
    assert!(wide.contains("   chase@dell\n"));
    assert!(wide.contains("   ──────────\n"));
    assert!(wide.contains("   Address: 192.168.1.42/24 (net0)\n"));
    let narrow = shown(&beside_duke(&dell(), Mode::True, 60));
    assert!(narrow.contains("   CPU: Intel Core i7-8550U (8) @ 4.00 GHz\n"));
    assert!(!narrow.contains("GPU"));
    // Cut to fit, rather than wrapped.
    let cramped = shown(&beside_duke(&dell(), Mode::True, 48));
    assert!(cramped.contains("   OS: Dukovo Server 0.12.0 (x86…\n"), "{cramped}");
}

#[test]
fn json_has_every_fact_with_a_source() {
    assert_eq!(
        json(&dell()),
        "{\n  \"user\": \"chase\",\n  \"host\": \"dell\",\n  \"os\": {\"name\": \"Dukovo Server\", \
         \"version\": \"0.12.0\", \"machine\": \"x86_64\"},\n  \"kernel\": {\"name\": \"Radix\", \
         \"version\": \"0.12.0\"},\n  \"uptime_seconds\": 274327,\n  \"bundles\": 14,\n  \"shell\": \
         {\"name\": \"Nukesh\", \"version\": \"0.12.0\", \"path\": \"/system/commands/nukesh\"},\n  \
         \"terminal\": {\"program\": \"consoled\", \"device\": \"pts/0\"},\n  \"cpu\": {\"name\": \
         \"Intel Core i7-8550U\", \"cpus\": 8, \"mhz\": 4000},\n  \"gpus\": [\"Intel UHD Graphics \
         620\"],\n  \"memory\": {\"used\": 2254857830, \"total\": 16642998272},\n  \"disk\": \
         {\"mount\": \"/\", \"used\": 40802189312, \"total\": 255550554112, \"type\": \"btrfs\"},\n  \
         \"addresses\": [{\"interface\": \"net0\", \"address\": \"192.168.1.42\", \"prefix\": \
         24}]\n}\n"
    );
    let bare = Facts { user: "a\"b".into(), host: "box".into(), ..Facts::default() };
    assert_eq!(
        json(&bare),
        "{\n  \"user\": \"a\\\"b\",\n  \"host\": \"box\",\n  \"gpus\": [],\n  \"addresses\": []\n}\n"
    );
}

#[test]
fn cuts_text_to_fit() {
    assert_eq!(cut("Radix 0.12.0", 20), "Radix 0.12.0");
    assert_eq!(cut("Radix 0.12.0", 12), "Radix 0.12.0");
    assert_eq!(cut("Radix 0.12.0", 8), "Radix 0…");
    assert_eq!(cut("Radix", 0), "…");
}
