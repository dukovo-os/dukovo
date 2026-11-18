#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The disk that holds Dukovo's root, for the lab tests that name it: a
# SATA disk is sda on the OptiPlex and an NVMe drive is nvme0n1 on the
# M75q, so no test writes either name. A test copies this file to the
# machine (services/copy-file.sh) and sources it as root:
#
#   . /users/lab/root-disk.sh
#
# It sets disk (sda, nvme0n1), part (the root's partition, sda3 or
# nvme0n1p3) and esp (the partition after the BIOS boot partition, the
# EFI system partition: sda2, nvme0n1p2). The lab image lays its disk out
# as BIOS boot, EFI system, root. A disk's name that ends in a digit takes
# a p before its partition numbers, as Linux names them.
part=$(sed -n 's/^disk //p' /system/live/spark/root)
case "$part" in
*[0-9]p[0-9]*)
	disk=${part%p*}
	esp=${disk}p2
	;;
*)
	disk=$(echo "$part" | sed 's/[0-9]*$//')
	esp=${disk}2
	;;
esac
