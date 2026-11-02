<!--
SPDX-FileCopyrightText: 2026 The Dukovo contributors
SPDX-License-Identifier: Apache-2.0 OR MIT
-->

Section: In every image
| [smb2](https://github.com/vdavid/smb2) 0.39.0 by David Veszelovszki, on [tokio](https://github.com/tokio-rs/tokio) 1.54.1 by the Tokio contributors and [socket2](https://github.com/rust-lang/socket2) 0.6.5, both patched for Radix, with the [RustCrypto](https://github.com/RustCrypto) project's aes, aes-gcm, ccm, cmac, hmac, md4, md-5, sha1, sha2 and pbkdf2, [lz4_flex](https://github.com/pseitz/lz4_flex) by Pascal Seitz and Arthur Silva, [bytes](https://github.com/tokio-rs/bytes), [futures-util](https://github.com/rust-lang/futures-rs), [num_enum](https://github.com/illicitonion/num_enum), [thiserror](https://github.com/dtolnay/thiserror) and [async-trait](https://github.com/dtolnay/async-trait) by David Tolnay, and the crates they use, from crates.io | smb(1)'s SMB 2 and 3: signing in with NTLM, signed and encrypted messages, and a share's files, which smb serves through `/devices/fuse` ([ADR 0227](docs/adr/0227-shares-of-windows-networks.md)); every one, with its version and license, is in the image's license manifest | MIT OR Apache-2.0 (smb2, socket2, the RustCrypto crates, futures-util, thiserror, async-trait); MIT (tokio, bytes, lz4_flex); BSD-3-Clause OR MIT OR Apache-2.0 (num_enum) |
