// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Dukovo's bundles as files (ADR 0222): the list of a bundle's files
//! and minisign's signature of it, the SBOM, and the zip a `.dkv` file
//! is. bundle(1) signs, packs and checks bundles with it on Dukovo, and
//! ob packs the catalog's bundles of ports with it on the build machine
//! (ADR 0370), so that both make the same files.

pub mod base64;
pub mod minisign;
pub mod sbom;
pub mod signed;
pub mod sums;
pub mod zip;

/// The manifest's name in a bundle's folder.
pub const MANIFEST: &str = "manifest.json";

/// A packed bundle's extension.
pub const EXTENSION: &str = "dkv";
