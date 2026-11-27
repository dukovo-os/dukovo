#!/bin/sh
# SPDX-FileCopyrightText: 2026 The Dukovo contributors
# SPDX-License-Identifier: Apache-2.0 OR MIT

# The machine's side of lab-catalog.sh, run as root: it adds the lab's catalog
# that lab-catalog.sh copied to /users/lab/lab-catalog as the catalog
# `labtest`, from /shared/lab-catalog, which bundled can read. The one a test
# before it added on the same boot is taken away first, as is its copy; an
# app installed from it stays bound to it, and is installed from it again.
#
#   sh /users/lab/lab-catalog-add.sh
set -eu

bundle catalog remove labtest > /dev/null 2>&1 || true
rm -rf /shared/lab-catalog
cp -r /users/lab/lab-catalog /shared/lab-catalog
chmod -R a+rX /shared/lab-catalog
bundle catalog add labtest file:///shared/lab-catalog/catalog /shared/lab-catalog/root.json
