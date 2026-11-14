#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT
#
# The views lab test's service program: what its view holds, said as
# lines that start with "lab-view:" (ADR 0473). The service's shell reads
# it from /system/settings, where the test puts it.

if [ -e /processes/1 ]; then echo "lab-view: sees process 1"; else echo "lab-view: sees no process 1"; fi
if [ -e /processes/$$ ]; then echo "lab-view: sees itself"; fi
if [ -e /devices/null ]; then echo "lab-view: has the null device"; fi
if [ -e /devices/kbd ]; then echo "lab-view: has the keyboard"; else echo "lab-view: has no keyboard"; fi
echo "lab-view: devices $(ls /devices | tr '\n' ' ')"
echo "lab-view: done"
sleep 30
