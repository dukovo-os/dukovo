// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! Chassis Identify (IPMI 2.0 section 28.5): the light or beeper that
//! lets a person find the machine in a rack.

use super::{NETFN_CHASSIS, Result, Transport, command};

/// Chassis Identify.
const CHASSIS_IDENTIFY: u8 = 0x04;

/// What the identify light is asked to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Identify {
    /// On for this many seconds, 1 to 255.
    Seconds(u8),
    /// On until it is turned off. The interval is sent as 0 and the force
    /// bit, bit 0 of the second byte, says it is ignored.
    ForceOn,
    /// Off: an interval of 0.
    Off,
}

/// Sets the identify light.
///
/// # Errors
///
/// If the BMC refuses the command, as one with no identify light does.
pub(crate) fn identify(link: &mut dyn Transport, what: Identify) -> Result<()> {
    let data: &[u8] = &match what {
        Identify::Seconds(seconds) => [seconds, 0],
        Identify::ForceOn => [0, 1],
        Identify::Off => [0, 0],
    };
    // The force byte is optional and a BMC of the 1.5 specification
    // refuses it, so only the force-on request sends it.
    let data = if what == Identify::ForceOn { data } else { &data[..1] };
    command(link, NETFN_CHASSIS, CHASSIS_IDENTIFY, data).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::super::sim::Sim;
    use super::*;

    #[test]
    fn identify_sends_the_interval_or_the_force_bit() {
        let mut bmc = Sim::new();
        identify(&mut bmc, Identify::Seconds(30)).unwrap();
        identify(&mut bmc, Identify::ForceOn).unwrap();
        identify(&mut bmc, Identify::Off).unwrap();
        assert_eq!(bmc.identifies, [vec![30], vec![0, 1], vec![0]]);
    }

    #[test]
    fn a_bmc_without_the_command_says_so() {
        let mut bmc = Sim::new();
        bmc.identify_refused = true;
        let error = identify(&mut bmc, Identify::Seconds(5)).unwrap_err();
        assert!(error.to_string().contains("invalid command"), "{error}");
    }
}
