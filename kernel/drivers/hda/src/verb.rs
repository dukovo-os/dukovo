// SPDX-FileCopyrightText: 2026 The Dukovo contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! What a codec is asked (High Definition Audio Specification 1.0a,
//! section 7.3): a verb for a node, as the 20 bits of a command after the
//! codec's address and the node's ID. A verb is either 12 bits with an
//! 8-bit payload or, for the converter format and the amplifiers, 4 bits
//! with a 16-bit payload.

/// A 12-bit verb with its 8-bit payload.
pub const fn v12(verb: u16, payload: u8) -> u32 {
    ((verb as u32) << 8) | payload as u32
}

/// A 4-bit verb with its 16-bit payload.
pub const fn v4(verb: u8, payload: u16) -> u32 {
    ((verb as u32) << 16) | payload as u32
}

/// Reads parameter `payload` ([`param`]).
pub const GET_PARAMETER: u16 = 0xf00;
/// Chooses a widget's input, by its place in the connection list.
pub const SET_CONNECTION_SELECT: u16 = 0x701;
/// Reads four (short form) or two (long form) entries of the
/// connection list, from the entry the payload names.
pub const GET_CONNECTION_LIST: u16 = 0xf02;
/// Sets a node's power state (0, D0, is fully on).
pub const SET_POWER_STATE: u16 = 0x705;
/// Reads a node's power state: the actual state in bits 7:4.
pub const GET_POWER_STATE: u16 = 0xf05;
/// Sets a converter's stream tag (bits 7:4) and first channel (3:0).
pub const SET_STREAM_CHANNEL: u16 = 0x706;
/// Sets a pin's control: [`PIN_OUT`], [`PIN_HP`].
pub const SET_PIN_CONTROL: u16 = 0x707;
/// Reads a pin's sense: bit 31 is set while something is plugged in.
pub const GET_PIN_SENSE: u16 = 0xf09;
/// Sets a pin's EAPD/BTL bits: [`EAPD`] turns an external amplifier on.
pub const SET_EAPD: u16 = 0x70c;
/// Reads a pin's configuration default, which the firmware set.
pub const GET_CONFIG_DEFAULT: u16 = 0xf1c;
/// Reads the function group's subsystem ID: the card's vendor and model.
pub const GET_SUBSYSTEM_ID: u16 = 0xf20;
/// Sets a converter's format (4-bit verb): the stream's format word.
pub const SET_FORMAT: u8 = 0x2;
/// Sets the index of a processing coefficient (4-bit verb): Realtek's
/// codecs keep settings in coefficients of node 0x20.
pub const SET_COEF_INDEX: u8 = 0x5;
/// Reads the coefficient at the index (4-bit verb).
pub const GET_PROC_COEF: u8 = 0xc;
/// Sets the coefficient at the index (4-bit verb).
pub const SET_PROC_COEF: u8 = 0x4;
/// Sets amplifiers' gain and mute (4-bit verb): [`amp`].
pub const SET_AMP: u8 = 0x3;

/// SET_PIN_CONTROL: the pin drives its output.
pub const PIN_OUT: u8 = 0x40;
/// SET_PIN_CONTROL: the pin drives headphones (its amplifier on).
pub const PIN_HP: u8 = 0x80;
/// SET_EAPD: the external amplifier is on.
pub const EAPD: u8 = 0x02;
/// GET_PIN_SENSE: something is plugged in.
pub const SENSE_PRESENT: u32 = 1 << 31;

/// SET_AMP's payload.
pub mod amp {
    /// The output amplifier.
    pub const OUTPUT: u16 = 1 << 15;
    /// The input amplifier (the one of the input [`index`] names).
    pub const INPUT: u16 = 1 << 14;
    /// The left channel.
    pub const LEFT: u16 = 1 << 13;
    /// The right channel.
    pub const RIGHT: u16 = 1 << 12;
    /// Muted.
    pub const MUTE: u16 = 1 << 7;

    /// The input amplifier of input `index` (bits 11:8).
    pub const fn index(index: u8) -> u16 {
        ((index & 0xf) as u16) << 8
    }
}

/// The parameters GET_PARAMETER reads (7.3.4).
pub mod param {
    /// The codec's vendor (bits 31:16) and device (15:0).
    pub const VENDOR_ID: u8 = 0x00;
    /// The codec's revision.
    pub const REVISION_ID: u8 = 0x02;
    /// A node's subordinates: the first (bits 23:16) and how many (7:0).
    pub const NODE_COUNT: u8 = 0x04;
    /// A function group's type (bits 7:0): [`super::FG_AUDIO`].
    pub const FUNCTION_TYPE: u8 = 0x05;
    /// A widget's capabilities: [`super::wcap`].
    pub const WIDGET_CAPS: u8 = 0x09;
    /// The rates (bits 11:0) and sample sizes (20:16) a converter takes.
    pub const PCM: u8 = 0x0a;
    /// The stream formats a converter takes: bit 0, PCM.
    pub const STREAM_FORMATS: u8 = 0x0b;
    /// A pin's capabilities: [`super::pincap`].
    pub const PIN_CAPS: u8 = 0x0c;
    /// The input amplifiers' capabilities: [`super::AmpCaps`].
    pub const IN_AMP_CAPS: u8 = 0x0d;
    /// How long the connection list is (bits 6:0) and whether it is in
    /// the long form (bit 7).
    pub const CONNECTION_LIST_LEN: u8 = 0x0e;
    /// The output amplifiers' capabilities: [`super::AmpCaps`].
    pub const OUT_AMP_CAPS: u8 = 0x12;
}

/// FUNCTION_TYPE: an audio function group.
pub const FG_AUDIO: u32 = 0x01;

/// WIDGET_CAPS bits.
pub mod wcap {
    /// The widget's type (bits 23:20).
    pub const fn kind(caps: u32) -> u32 {
        (caps >> 20) & 0xf
    }
    /// An output converter (a DAC).
    pub const OUTPUT: u32 = 0x0;
    /// A mixer: it adds its inputs.
    pub const MIXER: u32 = 0x2;
    /// A selector: it passes one of its inputs.
    pub const SELECTOR: u32 = 0x3;
    /// A pin: a jack, or a fixed speaker.
    pub const PIN: u32 = 0x4;
    /// Two channels rather than one.
    pub const STEREO: u32 = 1 << 0;
    /// It has input amplifiers.
    pub const IN_AMP: u32 = 1 << 1;
    /// It has an output amplifier.
    pub const OUT_AMP: u32 = 1 << 2;
    /// Its amplifiers' capabilities are its own, not the group's.
    pub const AMP_OVERRIDE: u32 = 1 << 3;
    /// Its formats are its own, not the group's.
    pub const FORMAT_OVERRIDE: u32 = 1 << 4;
    /// It has a connection list.
    pub const CONN_LIST: u32 = 1 << 8;
    /// It carries digital audio (S/PDIF, HDMI, DisplayPort).
    pub const DIGITAL: u32 = 1 << 9;
    /// It has power states of its own.
    pub const POWER: u32 = 1 << 10;
}

/// PIN_CAPS bits.
pub mod pincap {
    /// The pin senses whether something is plugged in.
    pub const PRESENCE: u32 = 1 << 2;
    /// The pin can drive headphones.
    pub const HP: u32 = 1 << 3;
    /// The pin can be an output.
    pub const OUTPUT: u32 = 1 << 4;
    /// The pin is HDMI.
    pub const HDMI: u32 = 1 << 7;
    /// The pin has an EAPD control.
    pub const EAPD: u32 = 1 << 16;
    /// The pin is DisplayPort.
    pub const DP: u32 = 1 << 24;
}

/// A pin's configuration default (7.3.3.31), which the firmware sets
/// to say what the pin is in this machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config(pub u32);

/// Config::connectivity: a jack.
pub const CONN_JACK: u32 = 0;
/// Config::connectivity: nothing is connected to the pin.
pub const CONN_NONE: u32 = 1;
/// Config::connectivity: a fixed device, such as a built-in speaker.
pub const CONN_FIXED: u32 = 2;

/// Config::device: line out.
pub const DEVICE_LINE_OUT: u32 = 0x0;
/// Config::device: a speaker.
pub const DEVICE_SPEAKER: u32 = 0x1;
/// Config::device: headphones.
pub const DEVICE_HP_OUT: u32 = 0x2;

impl Config {
    /// How the pin is connected: [`CONN_JACK`], [`CONN_NONE`],
    /// [`CONN_FIXED`] or both (3).
    pub const fn connectivity(self) -> u32 {
        self.0 >> 30
    }

    /// Where the pin is (bits 29:24): bits 5:4 say inside or outside the
    /// case, bits 3:0 which side.
    pub const fn location(self) -> u32 {
        (self.0 >> 24) & 0x3f
    }

    /// What is connected: [`DEVICE_LINE_OUT`], [`DEVICE_SPEAKER`],
    /// [`DEVICE_HP_OUT`] and others.
    pub const fn device(self) -> u32 {
        (self.0 >> 20) & 0xf
    }

    /// Whether the firmware says the jack cannot sense what is plugged
    /// in (bit 8, the first of the miscellaneous bits).
    pub const fn no_presence_detect(self) -> bool {
        self.0 & (1 << 8) != 0
    }

    /// The association (bits 7:4) the pin belongs to.
    pub const fn association(self) -> u32 {
        (self.0 >> 4) & 0xf
    }

    /// The pin's place in its association (bits 3:0).
    pub const fn sequence(self) -> u32 {
        self.0 & 0xf
    }
}

/// An amplifier's capabilities (7.3.4.10).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AmpCaps(pub u32);

impl AmpCaps {
    /// The step that is 0 dB (bits 6:0).
    pub const fn offset(self) -> u8 {
        (self.0 & 0x7f) as u8
    }

    /// The highest step (bits 14:8): 0 when the gain is fixed.
    pub const fn steps(self) -> u8 {
        ((self.0 >> 8) & 0x7f) as u8
    }

    /// The size of a step, in quarters of a decibel.
    pub const fn quarter_db(self) -> u32 {
        ((self.0 >> 16) & 0x7f) + 1
    }

    /// Whether it can be muted (bit 31).
    pub const fn mute(self) -> bool {
        self.0 & (1 << 31) != 0
    }
}

/// The rates of a converter's PCM parameter, bits 0 to 10 (bit 11,
/// 384 kHz, has no format word).
pub const PCM_RATES: [u32; 11] =
    [8000, 11025, 16000, 22050, 32000, 44100, 48000, 88200, 96000, 176400, 192000];

/// PCM: the converter takes 16-bit samples.
pub const PCM_16BIT: u32 = 1 << 17;

/// The format word for `rate` frames a second of 16-bit samples in
/// `channels` channels (3.7.1): the base rate (48 or 44.1 kHz), a
/// multiple and a divisor. `None` for a rate that has none.
pub fn format(rate: u32, channels: u8) -> Option<u16> {
    // (rate, 44.1 kHz base, multiple - 1, divisor - 1)
    const WORDS: [(u32, bool, u16, u16); 11] = [
        (8000, false, 0, 5),
        (11025, true, 0, 3),
        (16000, false, 0, 2),
        (22050, true, 0, 1),
        (32000, false, 1, 2),
        (44100, true, 0, 0),
        (48000, false, 0, 0),
        (88200, true, 1, 0),
        (96000, false, 1, 0),
        (176400, true, 3, 0),
        (192000, false, 3, 0),
    ];
    if channels == 0 || channels > 16 {
        return None;
    }
    let &(_, base, mult, div) = WORDS.iter().find(|word| word.0 == rate)?;
    let bits_16 = 1u16 << 4;
    Some(u16::from(base) << 14 | mult << 11 | div << 8 | bits_16 | u16::from(channels - 1))
}
