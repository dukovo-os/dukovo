// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Get Device ID (IPMI 2.0 section 20.1) and Get System GUID (section
//! 22.14): what the BMC is, which firmware it runs and which parts of the
//! specification it offers.

use super::{Error, NETFN_APP, Result, Transport, command};

/// Get Device ID.
const GET_DEVICE_ID: u8 = 0x01;
/// Get System GUID.
const GET_SYSTEM_GUID: u8 = 0x37;

/// What Get Device ID says (table 20-2).
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct DeviceId {
    pub device_id: u8,
    pub revision: u8,
    /// The firmware's major and minor revision, as 3.20 for 0x03, 0x20.
    pub firmware: String,
    /// Whether the firmware is being updated: it may not answer fully.
    pub updating: bool,
    /// The IPMI version, as 2.0.
    pub ipmi_version: String,
    /// The functions the device offers, from the additional device
    /// support byte.
    pub functions: Vec<&'static str>,
    /// The IANA enterprise number of the maker.
    pub manufacturer: u32,
    pub product: u16,
}

impl DeviceId {
    /// The maker's name, for the numbers that machines with a BMC mostly
    /// have; the number itself for others.
    pub(crate) fn manufacturer_name(&self) -> String {
        match self.manufacturer {
            2 => "IBM".into(),
            11 => "Hewlett-Packard".into(),
            343 => "Intel".into(),
            674 => "Dell".into(),
            5771 => "Cisco".into(),
            10876 => "Supermicro".into(),
            19046 => "Lenovo".into(),
            other => format!("enterprise {other}"),
        }
    }
}

/// The names of the bits of the additional device support byte.
const FUNCTIONS: [&str; 8] = [
    "sensor device",
    "SDR repository",
    "SEL",
    "FRU inventory",
    "event receiver",
    "event generator",
    "bridge",
    "chassis",
];

/// Asks the BMC what it is.
///
/// # Errors
///
/// If the BMC does not answer or its answer is too short.
pub(crate) fn device_id(link: &mut dyn Transport) -> Result<DeviceId> {
    let reply = command(link, NETFN_APP, GET_DEVICE_ID, &[])?;
    let [id, revision, major, minor, version, support, m0, m1, m2, p0, p1, ..] = reply[..] else {
        return Err(Error::Malformed("the device ID is cut short"));
    };
    // The minor revision is two BCD digits; the version is BCD with the
    // minor digit in the high nibble (0x02 is 2.0, 0x51 is 1.5).
    Ok(DeviceId {
        device_id: id,
        revision: revision & 0x0F,
        firmware: format!("{}.{:02X}", major & 0x7F, minor),
        updating: major & 0x80 != 0,
        ipmi_version: format!("{}.{}", version & 0x0F, version >> 4),
        functions: (0..8).filter(|bit| support >> bit & 1 == 1).map(|bit| FUNCTIONS[bit]).collect(),
        manufacturer: u32::from_le_bytes([m0, m1, m2 & 0x0F, 0]),
        product: u16::from_le_bytes([p0, p1]),
    })
}

/// The system's GUID as text, `8-4-4-4-12`, or `None` when the BMC has
/// none to give.
///
/// The BMC sends the sixteen bytes of RFC 4122's GUID in the opposite
/// order, least significant byte first (section 22.14).
///
/// # Errors
///
/// If the BMC does not answer, or the answer is not 16 bytes.
pub(crate) fn system_guid(link: &mut dyn Transport) -> Result<Option<String>> {
    let reply = match command(link, NETFN_APP, GET_SYSTEM_GUID, &[]) {
        Ok(reply) => reply,
        Err(Error::Completion(0xC1 | 0xD5)) => return Ok(None),
        Err(error) => return Err(error),
    };
    let Ok(bytes) = <[u8; 16]>::try_from(reply.get(..16).unwrap_or(&[])) else {
        return Err(Error::Malformed("the system GUID is not 16 bytes"));
    };
    let hex: Vec<String> = bytes.iter().rev().map(|byte| format!("{byte:02x}")).collect();
    let hex = hex.concat();
    Ok(Some(format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )))
}

#[cfg(test)]
mod tests {
    use super::super::sim::Sim;
    use super::*;

    #[test]
    fn the_device_id_is_decoded() {
        let mut bmc = Sim::new();
        let id = device_id(&mut bmc).unwrap();
        assert_eq!(id.firmware, "3.20");
        assert_eq!(id.ipmi_version, "2.0");
        assert_eq!(id.manufacturer, 674);
        assert_eq!(id.manufacturer_name(), "Dell");
        assert_eq!(id.product, 0x0100);
        assert_eq!(id.revision, 1);
        assert!(!id.updating);
        assert_eq!(id.functions, ["sensor device", "SDR repository", "SEL", "FRU inventory"]);
    }

    #[test]
    fn an_old_version_and_an_unknown_maker_read_as_they_are() {
        let mut bmc = Sim::new();
        bmc.device_id = vec![0x20, 0x81, 0x82, 0x05, 0x51, 0x00, 0x39, 0x30, 0xF0, 0, 0];
        let id = device_id(&mut bmc).unwrap();
        assert_eq!(id.firmware, "2.05");
        assert!(id.updating);
        assert_eq!(id.ipmi_version, "1.5");
        assert_eq!(id.manufacturer, 0x3039);
        assert_eq!(id.manufacturer_name(), "enterprise 12345");
    }

    #[test]
    fn a_short_device_id_is_refused() {
        let mut bmc = Sim::new();
        bmc.device_id = vec![0x20, 0x01];
        assert!(matches!(device_id(&mut bmc), Err(Error::Malformed(_))));
    }

    #[test]
    fn the_guid_comes_in_reverse_order() {
        let mut bmc = Sim::new();
        let guid = system_guid(&mut bmc).unwrap();
        assert_eq!(guid.as_deref(), Some("00112233-4455-6677-8899-aabbccddeeff"));
        bmc.guid = None;
        assert_eq!(system_guid(&mut bmc).unwrap(), None);
    }
}
