// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The Boot Error Record Table (signature `BERT`), which tells where the
//! firmware left the hardware errors it found before the operating system
//! started (ACPI Specification 6.5, section 18.3.1).
//!
//! The region holds a Generic Error Status Block; [`cper`] reads it.
//!
//! [`cper`]: crate::cper

use crate::{Error, Sdt, u32_at, u64_at};

/// The BERT's boot error region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bert {
    /// The most bytes the region holds.
    pub region_length: u32,
    /// The physical address of the region.
    pub region_address: u64,
}

impl Bert {
    /// Parses a `BERT` table.
    pub fn parse(sdt: &Sdt<'_>) -> Result<Bert, Error> {
        if &sdt.signature != b"BERT" {
            return Err(Error::BadSignature);
        }
        let body = sdt.body();
        // Boot Error Region Length at 36, Boot Error Region at 40.
        Ok(Bert {
            region_length: u32_at(body, 0).ok_or(Error::TooShort)?,
            region_address: u64_at(body, 4).ok_or(Error::TooShort)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::table;

    #[test]
    fn reads_the_boot_error_region() {
        let mut body = 0x1_0000u32.to_le_bytes().to_vec();
        body.extend_from_slice(&0x7fff_0000u64.to_le_bytes());
        let bytes = table(*b"BERT", 1, &body);
        let bert = Bert::parse(&Sdt::parse(0, &bytes).unwrap()).unwrap();
        assert_eq!(bert, Bert { region_length: 0x1_0000, region_address: 0x7fff_0000 });
    }

    #[test]
    fn refuses_the_wrong_table_and_a_short_one() {
        let bytes = table(*b"HEST", 1, &[0; 12]);
        assert_eq!(Bert::parse(&Sdt::parse(0, &bytes).unwrap()), Err(Error::BadSignature));
        let bytes = table(*b"BERT", 1, &[0; 8]);
        assert_eq!(Bert::parse(&Sdt::parse(0, &bytes).unwrap()), Err(Error::TooShort));
    }
}
