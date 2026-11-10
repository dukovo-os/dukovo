// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! A cloud image on AWS takes its setup from the instance metadata
//! service (ADR 0328). The machine's firmware names its maker Amazon EC2,
//! as AWS's machines do, and QEMU's user network, on 169.254.169.0/24,
//! runs the test's server for each connection to 169.254.169.254 port
//! 80: it answers as AWS's service, version 2, does, giving a token to a
//! PUT and refusing every other request without it. cloud-setup, with no
//! config disk, waits for the service, takes the token, the instance's
//! ID, the owner's SSH key and the user data, names the machine as the
//! user data says, makes the default account with the key, and runs the
//! user data's command.

use std::path::Path;

use super::super::*;

/// The server's folder, from the top of the tree, which QEMU runs in.
const DIR: &str = "out/test/cloud-metadata";

/// The test's network: QEMU's user network on the metadata service's
/// network, with the server at 169.254.169.254 port 80.
const NETDEV: &str = "user,id=n0,net=169.254.169.0/24,host=169.254.169.2,dns=169.254.169.3,\
                      dhcpstart=169.254.169.15,guestfwd=tcp:169.254.169.254:80-cmd:out/test/cloud-metadata/serve";

/// The instance's ID.
const INSTANCE: &str = "i-0123456789abcdef0";

/// The owner's key, which the service gives; the account's file holds it
/// whole.
const KEY: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIDukovoCloudMetadataTestKeyOnly metadata-owner";

pub const CASE: Case = Case {
    name: "cloud-metadata",
    about: "A cloud image on AWS: the firmware names Amazon EC2, and cloud-setup takes a token from the instance metadata service, version 2, then the instance, the owner's key and the user data, names the machine, makes the default account with the key and runs the user data's command (ADR 0328)",
    firmware: Firmware::Uefi,
    media: Media::Disk,
    cmdline: "console-sign-in=off",
    cpus: 2,
    memory: "1G",
    reboot: false,
    image: TestImage::Cloud,
    cpu_model: None,
    qemu: &[
        "-smbios",
        "type=1,manufacturer=Amazon EC2,product=t3.micro",
        "-netdev",
        NETDEV,
        "-device",
        "virtio-net-pci,netdev=n0",
    ],
    script: from_the_service,
    disks: &[],
    second: None,
};

/// The program QEMU runs for each connection: one HTTP request, answered
/// as AWS's instance metadata service, version 2, answers it, from the
/// files of its `site` folder, a folder's listing from its `index` file.
const SERVER: &str = "#!/bin/sh
# One HTTP request a connection, as AWS's instance metadata service answers.
cd \"$(dirname \"$0\")/site\" || exit 1
read -r method path rest
cr=$(printf '\\r')
token=
while IFS= read -r line; do
  case \"$line\" in \"\" | \"$cr\") break ;; esac
  case \"$line\" in
    [Xx]-[Aa]ws-[Ee]c2-[Mm]etadata-[Tt]oken:*) token=$(printf '%s' \"${line#*:}\" | tr -d ' \\r') ;;
  esac
done
answer() {
  printf 'HTTP/1.0 %s\\r\\nContent-Length: %s\\r\\nConnection: close\\r\\n\\r\\n' \"$1\" \\
    \"$(printf '%s' \"$2\" | wc -c | tr -d ' ')\"
  printf '%s' \"$2\"
}
if [ \"$method\" = PUT ] && [ \"$path\" = /latest/api/token ]; then
  answer '200 OK' TOKEN-42
  exit 0
fi
if [ \"$token\" != TOKEN-42 ]; then
  answer '401 Unauthorized' ''
  exit 0
fi
path=${path#/}
case \"$path\" in *..* | \"\") path=missing ;; esac
[ -d \"$path\" ] && path=\"$path/index\"
if [ \"$method\" = GET ] && [ -f \"$path\" ]; then
  answer '200 OK' \"$(cat \"$path\")\"
else
  answer '404 Not Found' ''
fi
";

/// The server and what it serves, in [`DIR`].
fn site() -> Result<(), String> {
    let dir = Path::new(DIR);
    if dir.exists() {
        std::fs::remove_dir_all(dir).map_err(|err| format!("removing {DIR}: {err}"))?;
    }
    let files: [(&str, &str); 6] = [
        ("serve", SERVER),
        ("site/latest/meta-data/instance-id", INSTANCE),
        ("site/latest/meta-data/local-hostname", "ip-169-254-169-15.ec2.internal"),
        ("site/latest/meta-data/public-keys/index", "0=owner"),
        ("site/latest/meta-data/public-keys/0/openssh-key", KEY),
        (
            "site/latest/user-data",
            "#cloud-config\nhostname: metal\nruncmd:\n  - echo \"metadata runcmd $((40 + 2))\"\n",
        ),
    ];
    for (name, text) in files {
        let path = dir.join(name);
        let made = std::fs::create_dir_all(path.parent().expect("in the folder"))
            .and_then(|()| std::fs::write(&path, text));
        made.map_err(|err| format!("writing {}: {err}", path.display()))?;
    }
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(dir.join("serve"), std::fs::Permissions::from_mode(0o755))
        .map_err(|err| format!("making {DIR}/serve runnable: {err}"))
}

/// The machine set up from the service.
fn from_the_service(vm: &mut Vm, _case: &Case) -> Result<(), String> {
    // Written before the machine has booted far enough to ask.
    site()?;
    vm.expect("cloud-setup: on AWS: asking its metadata service")?;
    vm.expect(&format!("cloud-setup: instance {INSTANCE} from AWS's instance metadata service"))?;
    vm.expect("cloud-setup: naming the machine metal")?;
    vm.expect("cloud-setup: making an administrator's account, dukovo, with 1 SSH key(s)")?;
    vm.expect("metadata runcmd 42")?;
    vm.expect(&format!("cloud-setup: instance {INSTANCE} set up\n"))?;
    vm.expect_screen(&[&format!("{RELEASE} {VERSION} (tty1)"), "# "])?;
    vm.type_text("clear; hostname; cat /users/dukovo/.ssh/authorized_keys\n")?;
    vm.expect_screen(&["metal", "metadata-owner"])?;
    vm.type_text("poweroff\n")?;
    vm.expect("spark: asked to power off the machine")?;
    vm.expect_exit(QEMU_EXIT_POWER_OFF)
}
