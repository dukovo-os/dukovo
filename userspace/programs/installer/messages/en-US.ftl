# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# Every message dukovo-install's screens show, in English (United
# States), in Fluent's syntax (projectfluent.org). A translation is a copy
# of this file named for its language, with each message's text
# translated and its ids and { $variables } left as they are (ADR 0310).

## Around every page

starting = Checking the system release on the install stick…
title = Install Dukovo { $version }
step-of = Step { $step } of { $steps } · { $name }
more = … more
hints = Tab move · Space select · Enter next · Esc back · F1 help · Alt+F2 shell
hints-installing = L shows the log · the shell on Alt+F2 keeps working
hints-done = Enter restarts · L shows the log
hints-failed = L shows the log · the shell on Alt+F2 can look further

page-welcome = Welcome
page-setup-file = Install from a setup file
page-network = Network and name
page-disk = Disk
page-you = You
page-review = Review
page-installing = Installing Dukovo
page-done = Dukovo is installed
page-failed = The install did not finish

button-live = Live console
button-back = Back
button-next = Next
button-load = Load
button-install = Install
button-help = Help
button-restart = Restart
button-log = Show log
button-save-setup = Save setup file to the stick

## Welcome

welcome-intro = This installs Dukovo { $version } on this computer. Nothing on any disk changes until you choose Install on the last step.
welcome-language = Language
language-english = English (United States)
welcome-language-note = more languages come in a later release
welcome-keyboard = Keyboard layout
welcome-try = Try your keyboard
welcome-try-hint = type here to check the layout
welcome-how = How do you want to install?
welcome-steps = Step by step
welcome-steps-note = answer five short screens
welcome-file = From a setup file
welcome-file-note = install.json on this stick's EFI partition

## Setup file

setup-intro = A setup file (install.json) answers every question, so the install runs by itself once you confirm it. It is looked for at the top of this stick's EFI partition, which any computer can write.
setup-none = No setup file was found on this stick.
setup-ready = found, and every answer checks
setup-on-stick = { $file } on this USB stick
setup-no-esp = This stick has no EFI partition for a setup file.
setup-saved = Saved as install.json on the stick, with the passwords' hashes.
setup-saved-no-hashes = Saved as install.json on the stick, without the passwords' hashes: add them (dukovo-install --hash-password) before it installs.

## Network

network-cards = Network cards found:
network-no-cards = No network card was found. An install needs none.
network-cable = cable in
network-cable-speed = cable in, { $speed } Mb/s
network-no-cable = no cable
network-dhcp = Automatic (DHCP)
network-dhcp-note = asks the network for an address
network-dhcp-got = got { $address }
network-fixed = Fixed address
network-fixed-note = type it in below
network-address = Address/prefix
network-gateway = Gateway
network-dns = DNS servers
network-hostname = Computer name
network-zone = Time zone
network-zone-note = Left and Right, or type the start of its name
network-offline = An offline install needs no network: the installed system uses it for updates and features.

## Disk

disk-which = Which disk should Dukovo go on?
disk-stick = the install stick, can't be chosen
disk-too-small = too small: the install needs { $size }
disk-none = No disk can take Dukovo: each is the stick or too small.
disk-empty = no partitions
disk-windows = { $count ->
    [one] Windows, 1 partition
   *[other] Windows, { $count } partitions
}
disk-dukovo = { $count ->
    [one] Dukovo, 1 partition
   *[other] Dukovo, { $count } partitions
}
disk-linux = { $count ->
    [one] Linux, 1 partition
   *[other] Linux, { $count } partitions
}
disk-partitions = { $count ->
    [one] 1 partition
   *[other] { $count } partitions
}
disk-how = How should it be set up?
disk-guided = Erase the disk and use the guided layout
disk-guided-note = recommended
disk-swap = Swap size (GiB)
disk-swap-note = Swap the size of memory, at most 8 GiB, is suggested; 0 makes none.
disk-erase-windows = Erasing { $disk } deletes Windows and everything on it.
disk-erase = Erasing { $disk } deletes everything on it.
disk-untouched = { $disks } stays untouched: format it later with rdisk and use it under /volumes.

## You

you-intro = The administrator account. It signs in like any other and can elevate to change the system.
you-name = Your name
you-user = User name
you-password = Password
you-repeat = Repeat password
you-short = ! This password is short. You can keep it.
you-keep = Keep the password hashes in answers.json
you-keep-note = so a repeat install sets the same passwords
you-others = Other people (optional):
you-none-yet = none yet
you-person-note = Delete removes
you-add = Add a person…
you-mismatch = The two passwords differ: type the same password twice.
you-taken = { $user } is taken: choose another user name.
person-title = Add a person
hint-person = Tab moves · Enter adds · Esc cancels

## Review

review-intro = This is the only step that changes this computer. Nothing has been written yet.
review-erase = Erase
review-erase-windows = (Windows and all its files are deleted)
review-erase-all = (everything on it is deleted)
review-create = Create
review-on = on { $disk }:
review-bios-boot = BIOS boot
review-efi = EFI system
review-efi-note = FAT32
review-swap = Swap
review-swap-note = swap
review-root = Dukovo root
review-root-note = Btrfs, at /
review-subvolumes = Btrfs subvolumes: @system.a @settings @state @applications @users @shared @snapshots
review-install = Install
review-release = Dukovo { $version }, the stick's signed system release
review-grub = GRUB for UEFI and BIOS
review-settings = Settings
review-dhcp = { $card } DHCP
review-keyboard = keyboard { $layout }
review-accounts = Accounts
review-admin = { $user } ({ $name }), administrator
review-untouched = Untouched
review-confirm = Type { $disk } to confirm
review-type = Type { $disk } in the box to confirm that it is erased.

## The install's steps, as the screens list them and the console prints them

step-release = Checking the signatures of the system release
step-disk = Checking { $disk }
step-partition = Partitioning { $disk }
step-format = Making the filesystems
step-subvolumes = Making the subvolumes
step-copy = Copying Dukovo { $version } into its slot, checking every file
step-settings = Writing the settings and accounts
step-grub = Installing GRUB for UEFI and BIOS
step-save = Saving the answers and the install log
installed-line = Dukovo { $version } is installed on { $disk }. Remove the install stick, then restart.
done-saved = Your answers were saved as /system/settings/install/answers.json.
done-remove = Remove the install stick, then restart.
failed-log = Nothing more is written. The log is { $log }: L shows it, and the shell on Alt+F2 can read it whole.

## Help

help-title = Help
hint-close = Esc closes
hint-log = Up, Down, Page Up and Page Down scroll · Esc closes
help-welcome =
    Tab and Shift+Tab, or Up and Down, move between the fields and the buttons. Space chooses, Enter goes on, Esc goes back.
    Left and Right change the keyboard layout; the keys type in it at once, so try them in the box below it.
    Step by step asks five short screens. A setup file answers them all: install.json on this stick's EFI partition, as the Done page saves one.
    Live console leaves the installer for a shell, where dukovo-install starts it again.
help-setup-file =
    A setup file is an answers file (install-answers(5)) called install.json at the top of the stick's EFI partition.
    Load checks every answer and shows the review; nothing is written before you confirm it there.
help-network =
    The installed system asks DHCP for an address on every card unless a card is given a fixed one.
    The computer's name is letters, digits and -, not at its start or end.
    The time zone sets the clock the system shows; it keeps UTC inside.
help-disk =
    The guided layout erases the disk and makes a BIOS boot partition, an EFI system partition, swap, and the Dukovo root on Btrfs, which starts under UEFI and a BIOS alike.
    Other disks stay as they are. The install stick cannot be chosen.
help-you =
    The administrator signs in like anyone and elevates to change the system.
    The installed system keeps your answers without the passwords' hashes unless you ask it to keep them, for a repeat install.
    An empty password means none: anyone at the keyboard can then sign in as you.
help-review =
    Type the disk's name to confirm that it is erased, then Install.
    Each step's commands go to the install log, /tmp/install.log, which L shows during the install.
help-installing =
    The install runs each step with the system's own tools. L shows the log; Alt+F2 has a shell.
help-done =
    Remove the install stick, then restart: the computer starts Dukovo from the disk.
help-failed =
    The log names the step that failed and why. Nothing is written after a failure; the disk must be installed again.
