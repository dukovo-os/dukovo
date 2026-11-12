<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

Section: Build, test and documentation tools
| [Caddy](https://caddyserver.com) by Matthew Holt and the Caddy contributors, from the update server's Ubuntu packages, with certificates from [Let's Encrypt](https://letsencrypt.org) | Serving the catalog and the system's releases over https from the update server, which `ob catalog server setup` sets up ([ADR 0382](docs/adr/0382-the-update-server-publishes-what-the-signer-deploys-over-ssh.md), [ADR 0467](docs/adr/0467-the-update-server-serves-the-systems-releases-and-signs-only-their-snapshot-and-timestamp.md)); never in CI or an image | Apache-2.0 |
