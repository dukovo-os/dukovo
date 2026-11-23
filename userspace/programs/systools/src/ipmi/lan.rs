// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! The BMC's network settings: Get Channel Info (IPMI 2.0 section 22.24) to
//! find the first LAN channel, and Get and Set LAN Configuration
//! Parameters (section 23.1 and 23.2, parameters in table 23-4) for the
//! IP address, its source, mask, gateway, MAC address and VLAN.

use std::net::Ipv4Addr;

use super::{Error, NETFN_APP, NETFN_TRANSPORT, Result, Transport, command};

/// Get Channel Info.
const GET_CHANNEL_INFO: u8 = 0x42;
/// Set LAN Configuration Parameters.
const SET_LAN_CONFIGURATION: u8 = 0x01;
/// Get LAN Configuration Parameters.
const GET_LAN_CONFIGURATION: u8 = 0x02;

/// The channel medium of 802.3 LAN (table 6-3).
const MEDIUM_LAN: u8 = 0x04;
/// The channels with numbers 1 to 11 are the ones that carry a medium
/// (table 6-1); 0 is the primary IPMB and 15 the system interface.
const CHANNELS: std::ops::RangeInclusive<u8> = 1..=11;

// The parameters of table 23-4.
const SET_IN_PROGRESS: u8 = 0;
const IP_ADDRESS: u8 = 3;
const IP_ADDRESS_SOURCE: u8 = 4;
const MAC_ADDRESS: u8 = 5;
const SUBNET_MASK: u8 = 6;
const DEFAULT_GATEWAY: u8 = 12;
const VLAN_ID: u8 = 20;

/// The values of the "set in progress" parameter (table 23-4, parameter
/// 0): the settings are being changed, or the change is to be applied.
const SET_COMPLETE: u8 = 0;
const SET_STARTED: u8 = 1;
const COMMIT_WRITE: u8 = 2;

/// Where the BMC's address comes from (table 23-4, parameter 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IpSource {
    Unspecified,
    Static,
    Dhcp,
    /// Set by the BIOS or system software.
    Bios,
    /// Another protocol, by its number.
    Other(u8),
}

impl IpSource {
    fn of(code: u8) -> IpSource {
        match code & 0x0F {
            0 => IpSource::Unspecified,
            1 => IpSource::Static,
            2 => IpSource::Dhcp,
            3 => IpSource::Bios,
            other => IpSource::Other(other),
        }
    }

    /// The word for the source, as the JSON has it.
    pub(crate) fn name(self) -> String {
        match self {
            IpSource::Unspecified => "unspecified".into(),
            IpSource::Static => "static".into(),
            IpSource::Dhcp => "dhcp".into(),
            IpSource::Bios => "bios".into(),
            IpSource::Other(code) => format!("other {code}"),
        }
    }
}

/// The settings of a LAN channel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Lan {
    pub channel: u8,
    pub source: IpSource,
    pub address: Option<Ipv4Addr>,
    pub mask: Option<Ipv4Addr>,
    pub gateway: Option<Ipv4Addr>,
    pub mac: Option<[u8; 6]>,
    /// The VLAN's number, when the BMC's traffic is tagged.
    pub vlan: Option<u16>,
}

/// The first channel whose medium is 802.3 LAN, or `None` for a BMC with
/// no network port.
///
/// # Errors
///
/// If the transport fails. A channel the BMC says it does not have is
/// passed over.
pub(crate) fn find_lan_channel(link: &mut dyn Transport) -> Result<Option<u8>> {
    for channel in CHANNELS {
        match command(link, NETFN_APP, GET_CHANNEL_INFO, &[channel]) {
            Ok(reply) if reply.get(1).is_some_and(|medium| medium & 0x7F == MEDIUM_LAN) => {
                return Ok(Some(channel));
            }
            Ok(_) | Err(Error::Completion(_)) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(None)
}

/// One parameter's bytes, without the revision byte; `None` when the BMC
/// does not have the parameter (completion code 0x80).
fn parameter(link: &mut dyn Transport, channel: u8, number: u8) -> Result<Option<Vec<u8>>> {
    match command(link, NETFN_TRANSPORT, GET_LAN_CONFIGURATION, &[channel & 0x0F, number, 0, 0]) {
        Ok(reply) => Ok(Some(reply.get(1..).unwrap_or(&[]).to_vec())),
        Err(Error::Completion(0x80)) => Ok(None),
        Err(error) => Err(error),
    }
}

fn address(link: &mut dyn Transport, channel: u8, number: u8) -> Result<Option<Ipv4Addr>> {
    Ok(parameter(link, channel, number)?
        .and_then(|bytes| <[u8; 4]>::try_from(bytes.get(..4)?).ok())
        .map(Ipv4Addr::from))
}

/// Reads the settings of `channel`.
///
/// # Errors
///
/// If the BMC does not answer. Parameters it does not have read as
/// `None`.
pub(crate) fn read_lan(link: &mut dyn Transport, channel: u8) -> Result<Lan> {
    let source = parameter(link, channel, IP_ADDRESS_SOURCE)?
        .and_then(|bytes| bytes.first().copied())
        .map_or(IpSource::Unspecified, IpSource::of);
    let mac = parameter(link, channel, MAC_ADDRESS)?
        .and_then(|bytes| <[u8; 6]>::try_from(bytes.get(..6)?).ok());
    // Parameter 20: the low 8 bits of the VLAN's number, then bits 3:0 its
    // high 4 and bit 7 whether it is enabled.
    let vlan = parameter(link, channel, VLAN_ID)?.and_then(|bytes| {
        let (&low, &high) = (bytes.first()?, bytes.get(1)?);
        (high & 0x80 != 0).then(|| u16::from(low) | u16::from(high & 0x0F) << 8)
    });
    Ok(Lan {
        channel,
        source,
        address: address(link, channel, IP_ADDRESS)?,
        mask: address(link, channel, SUBNET_MASK)?,
        gateway: address(link, channel, DEFAULT_GATEWAY)?,
        mac,
        vlan,
    })
}

fn set_parameter(link: &mut dyn Transport, channel: u8, number: u8, data: &[u8]) -> Result<()> {
    let mut request = vec![channel & 0x0F, number];
    request.extend_from_slice(data);
    command(link, NETFN_TRANSPORT, SET_LAN_CONFIGURATION, &request).map(|_| ())
}

/// Runs `change` between the "set in progress" and "set complete" writes
/// the specification asks for (parameter 0, section 23.1): another
/// writer's change in progress is not broken into, and a BMC that wants
/// the change committed is told to.
fn in_progress(
    link: &mut dyn Transport,
    channel: u8,
    change: impl FnOnce(&mut dyn Transport) -> Result<()>,
) -> Result<()> {
    let state = parameter(link, channel, SET_IN_PROGRESS)?;
    if state.and_then(|bytes| bytes.first().copied()).is_some_and(|byte| byte & 0x03 == SET_STARTED)
    {
        return Err(Error::Refused(
            "another change to the BMC's network settings is in progress".into(),
        ));
    }
    set_parameter(link, channel, SET_IN_PROGRESS, &[SET_STARTED])?;
    let changed = change(link).and_then(|()| {
        // Commit write is optional: a BMC that has no use for it refuses
        // with 0x80 or 0xC9, and the change stands.
        match set_parameter(link, channel, SET_IN_PROGRESS, &[COMMIT_WRITE]) {
            Err(Error::Completion(0x80 | 0xC9)) => Ok(()),
            other => other,
        }
    });
    // The state goes back to complete whether the change worked or not, so
    // that a failed change does not leave the settings locked; the
    // change's own error is the one to report.
    let finished = set_parameter(link, channel, SET_IN_PROGRESS, &[SET_COMPLETE]);
    changed.and(finished)
}

/// Gives the BMC a static `address` with `mask`, and a default `gateway`
/// when one is given.
///
/// # Errors
///
/// If another change is in progress or the BMC refuses a value.
pub(crate) fn set_static(
    link: &mut dyn Transport,
    channel: u8,
    address: Ipv4Addr,
    mask: Ipv4Addr,
    gateway: Option<Ipv4Addr>,
) -> Result<()> {
    in_progress(link, channel, |link| {
        // Source 1 is static: a BMC that gets its address by DHCP takes
        // no address of its own until it is told to stop.
        set_parameter(link, channel, IP_ADDRESS_SOURCE, &[0x01])?;
        set_parameter(link, channel, IP_ADDRESS, &address.octets())?;
        set_parameter(link, channel, SUBNET_MASK, &mask.octets())?;
        match gateway {
            Some(gateway) => set_parameter(link, channel, DEFAULT_GATEWAY, &gateway.octets()),
            None => Ok(()),
        }
    })
}

/// Has the BMC take its address by DHCP.
///
/// # Errors
///
/// If another change is in progress or the BMC refuses.
pub(crate) fn set_dhcp(link: &mut dyn Transport, channel: u8) -> Result<()> {
    in_progress(link, channel, |link| set_parameter(link, channel, IP_ADDRESS_SOURCE, &[0x02]))
}

#[cfg(test)]
mod tests {
    use super::super::sim::Sim;
    use super::*;

    #[test]
    fn the_first_lan_channel_is_found_by_its_medium() {
        let mut bmc = Sim::new();
        // Channel 1 is a serial port, 2 does not exist, 3 and 4 are LANs.
        bmc.channels = vec![(1, 0x05), (3, 0x04), (4, 0x04)];
        assert_eq!(find_lan_channel(&mut bmc).unwrap(), Some(3));
        bmc.channels = vec![(1, 0x05)];
        assert_eq!(find_lan_channel(&mut bmc).unwrap(), None);
    }

    #[test]
    fn the_settings_are_read_from_their_parameters() {
        let mut bmc = Sim::new();
        let lan = read_lan(&mut bmc, 1).unwrap();
        assert_eq!(lan.channel, 1);
        assert_eq!(lan.source, IpSource::Static);
        assert_eq!(lan.address, Some(Ipv4Addr::new(10, 1, 2, 3)));
        assert_eq!(lan.mask, Some(Ipv4Addr::new(255, 255, 255, 0)));
        assert_eq!(lan.gateway, Some(Ipv4Addr::new(10, 1, 2, 1)));
        assert_eq!(lan.mac, Some([0x00, 0x11, 0x22, 0x33, 0x44, 0x55]));
        assert_eq!(lan.vlan, Some(0x123));
    }

    #[test]
    fn a_disabled_vlan_and_missing_parameters_read_as_none() {
        let mut bmc = Sim::new();
        bmc.lan.insert(20, vec![0x7B, 0x00]);
        bmc.lan.remove(&12);
        let lan = read_lan(&mut bmc, 1).unwrap();
        assert_eq!(lan.vlan, None);
        assert_eq!(lan.gateway, None);
    }

    #[test]
    fn the_sources_are_named() {
        assert_eq!(IpSource::of(0x02), IpSource::Dhcp);
        assert_eq!(IpSource::of(0x03).name(), "bios");
        assert_eq!(IpSource::of(0x14), IpSource::Other(4));
        assert_eq!(IpSource::Static.name(), "static");
    }

    #[test]
    fn a_static_address_is_set_between_in_progress_and_complete() {
        let mut bmc = Sim::new();
        bmc.lan.insert(4, vec![0x02]);
        let (address, mask, gateway) = (
            Ipv4Addr::new(192, 168, 7, 9),
            Ipv4Addr::new(255, 255, 0, 0),
            Ipv4Addr::new(192, 168, 7, 1),
        );
        set_static(&mut bmc, 1, address, mask, Some(gateway)).unwrap();
        let numbers: Vec<(u8, Vec<u8>)> = bmc.lan_sets.clone();
        assert_eq!(
            numbers,
            [
                (0, vec![1]),
                (4, vec![1]),
                (3, vec![192, 168, 7, 9]),
                (6, vec![255, 255, 0, 0]),
                (12, vec![192, 168, 7, 1]),
                (0, vec![2]),
                (0, vec![0]),
            ]
        );
        let lan = read_lan(&mut bmc, 1).unwrap();
        assert_eq!(lan.source, IpSource::Static);
        assert_eq!(lan.address, Some(address));
        assert_eq!(lan.gateway, Some(gateway));
    }

    #[test]
    fn dhcp_sets_the_source_and_nothing_else() {
        let mut bmc = Sim::new();
        set_dhcp(&mut bmc, 1).unwrap();
        let sets: Vec<u8> = bmc.lan_sets.iter().map(|(number, _)| *number).collect();
        assert_eq!(sets, [0, 4, 0, 0]);
        assert_eq!(read_lan(&mut bmc, 1).unwrap().source, IpSource::Dhcp);
    }

    #[test]
    fn a_change_in_progress_is_not_broken_into() {
        let mut bmc = Sim::new();
        bmc.lan.insert(0, vec![0x01]);
        let error = set_dhcp(&mut bmc, 1).unwrap_err();
        assert!(error.to_string().contains("in progress"), "{error}");
        assert!(bmc.lan_sets.is_empty());
    }

    #[test]
    fn a_refused_value_still_ends_the_set_in_progress() {
        let mut bmc = Sim::new();
        bmc.refuse_lan_parameter = Some(3);
        let result =
            set_static(&mut bmc, 1, Ipv4Addr::new(10, 0, 0, 1), Ipv4Addr::new(255, 0, 0, 0), None);
        assert!(matches!(result, Err(Error::Completion(0xCC))));
        assert_eq!(bmc.lan_sets.last(), Some(&(0, vec![0])));
    }

    #[test]
    fn a_bmc_that_has_no_commit_write_keeps_the_change() {
        let mut bmc = Sim::new();
        bmc.no_commit_write = true;
        set_dhcp(&mut bmc, 1).unwrap();
        assert_eq!(read_lan(&mut bmc, 1).unwrap().source, IpSource::Dhcp);
    }
}
