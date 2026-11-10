#!/bin/bash
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The cloud image in VirtualBox (ADR 0329). CI's VirtualBox job runs it on
# the VMDK that `ob image --cloud` writes, on a GitHub runner where
# VirtualBox is installed and KVM's modules are unloaded; it runs on any
# Linux machine with VirtualBox 7 and genisoimage.
#
#   tools/virtualbox/cloud-boot.sh <dukovo-VERSION-x86_64-cloud.vmdk>
#
# It makes a NoCloud config disc labelled cidata, ISO 9660, with a key of
# its own, a machine name and a command, and a VirtualBox machine of the
# image under UEFI, with the ICH9 chipset: the disk and the disc on its
# SATA controller, an
# Intel PRO/1000 card on NAT with port 2222 of the loopback forwarded to
# the machine's 22, and COM1 written to serial.log. Then:
#
# - cloud-setup sets the machine up from the disc, as its log says;
# - the host's ssh signs in as dukovo with the key, and elevates without
#   a password;
# - VirtualBox's ACPI power button powers the machine off, every service
#   stopped first.
#
# The work goes to $RUNNER_TEMP/virtualbox, or a folder in /tmp; on a
# failure the end of serial.log is shown.

set -euo pipefail

vmdk=$(realpath "${1:?usage: cloud-boot.sh <cloud.vmdk>}")
work=${RUNNER_TEMP:-/tmp}/virtualbox
name=dukovo-cloud
instance=vbox-ci-$(date +%s)
mkdir -p "$work/cidata"
serial=$work/serial.log
rm -f "$serial" "$work/key" "$work/key.pub" "$work/cidata.iso"

# VirtualBox's own commands, as root, whose drivers they are.
vbox() { sudo VBoxManage "$@"; }

# Waits up to $1 seconds for $2 in the serial log.
wait_for() {
  local seconds=$1 text=$2
  for _ in $(seq "$seconds"); do
    if sudo grep -qF -- "$text" "$serial" 2>/dev/null; then
      echo "seen: $text"
      return 0
    fi
    sleep 1
  done
  echo "::error::not seen in ${seconds} s: $text"
  return 1
}

# Shows the end of the serial log and takes the machine away.
finish() {
  local status=$?
  if [ "$status" -ne 0 ] && [ -f "$serial" ]; then
    echo '--- the end of the serial log ---'
    sudo tail -n 80 "$serial" || true
  fi
  # VirtualBox writes the log as root: CI keeps it on a failure.
  sudo chmod a+r "$serial" 2>/dev/null || true
  vbox controlvm "$name" poweroff >/dev/null 2>&1 || true
  vbox unregistervm "$name" --delete >/dev/null 2>&1 || true
  exit "$status"
}
trap finish EXIT

# The config disc: the key, the name and a command.
ssh-keygen -q -t ed25519 -N '' -C vbox-ci -f "$work/key"
cat >"$work/cidata/meta-data" <<EOF
instance-id: $instance
local-hostname: vbox-ci.example.org
public-keys:
  - $(cat "$work/key.pub")
EOF
cat >"$work/cidata/user-data" <<'EOF'
#cloud-config
runcmd:
  - echo "virtualbox runcmd $((40 + 2))"
EOF
genisoimage -quiet -output "$work/cidata.iso" -volid cidata -joliet -rock \
  "$work/cidata/meta-data" "$work/cidata/user-data"

# The machine. Its chipset is the ICH9: the kernel finds PCI devices
# through PCI Express's configuration space, which VirtualBox describes
# (ACPI's MCFG) with the ICH9 and not with its default, the PIIX3.
vbox createvm --name "$name" --ostype Linux_64 --basefolder "$work" --register
vbox modifyvm "$name" --memory 2048 --cpus 2 --firmware efi --chipset ich9 --ioapic on \
  --nic1 nat --nictype1 82540EM --natpf1 "ssh,tcp,127.0.0.1,2222,,22" \
  --uart1 0x3F8 4 --uartmode1 file "$serial"
vbox storagectl "$name" --name SATA --add sata --controller IntelAhci --portcount 2
vbox storageattach "$name" --storagectl SATA --port 0 --device 0 --type hdd --medium "$vmdk"
vbox storageattach "$name" --storagectl SATA --port 1 --device 0 --type dvddrive --medium "$work/cidata.iso"
vbox startvm "$name" --type headless

wait_for 600 "cloud-setup: instance $instance from /volumes/"
wait_for 60 "cloud-setup: naming the machine vbox-ci"
wait_for 60 "cloud-setup: making an administrator's account, dukovo, with 1 SSH key(s)"
wait_for 120 "virtualbox runcmd 42"
wait_for 60 "cloud-setup: instance $instance set up"
wait_for 60 "guest-agent: no guest agent channel"

# The host signs in with the disc's key, and elevates without a password.
ssh_in() {
  ssh -F /dev/null -p 2222 -i "$work/key" -o IdentitiesOnly=yes -o BatchMode=yes \
    -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR \
    -o ConnectTimeout=20 dukovo@127.0.0.1 "$@"
}
for _ in $(seq 30); do
  if ssh_in true; then break; fi
  sleep 2
done
uid=$(ssh_in elevate id -u)
if [ "$uid" != 0 ]; then
  echo "::error::dukovo did not elevate without a password: $uid"
  exit 1
fi
echo "signed in over ssh and elevated: uid $uid"
ssh_in hostname | grep -qx vbox-ci

# The power button powers it off cleanly.
vbox controlvm "$name" acpipowerbutton
wait_for 120 "spark: every service has stopped"
for _ in $(seq 60); do
  if vbox showvminfo "$name" --machinereadable | grep -qx 'VMState="poweroff"'; then
    echo "VirtualBox: the machine powered off"
    exit 0
  fi
  sleep 1
done
echo "::error::the machine did not power off"
exit 1
