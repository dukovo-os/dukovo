<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

Section: Projects whose ideas we follow
- [python-tuf](https://github.com/theupdateframework/python-tuf), the
  reference TUF implementation: the TUF client's order of checks follows
  its `ngclient`, the trusted set it keeps for one update, how it walks
  delegations, quotes role names into file names and URLs, and the size
  limits it sets
  ([ADR 0380](docs/adr/0380-tuf-checks-what-catalogs-and-updates-say-with-a-client-of-our-own.md)).
