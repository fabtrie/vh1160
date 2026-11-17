//! Command encoding and reply parsing, without any I/O.
//!
//! | Command | Reply | Meaning |
//! |---|---|---|
//! | `t` | `t D D D D` | version: firmware D1.D2, hardware revision D3·10+D4 |
//! | `w` | `w VVVV VVVV VVVV AAAA IIIII d` | KL30, KL15, KL31', analog in (V·100), current (A·10000), digital in |
//! | `s` | `s o f f` | overload digit, two raw error-flag bytes |
//! | `R` `S` / `T` `U` / `V` `W` | | KL30 / KL15 / KL31 relay open / close |
//! | `v####` | | VBatt, V·100 |
//! | `i####` `h` / `g` | | ground offset value + enable / off |
//! | `I####` `H` / `G` | | tester bus ground offset value + enable / off (rev ≥ 2) |
//! | `n####` `m` / `l` | | LIN recessive limit value + enable / off |
//! | `A####` | | analog output, V·100 (0 = off) |
//! | `D` / `E` | | digital output low / high |
//! | `L` `M` `N` `O` | | LINcab supply mode 0…3 |
//! | `c` p h l | | bus faults: interrupt, CAN_H, CAN_L digits (CAN_H→CAN_L is `80`) |
//! | `Z` | | safe state (rev ≥ 2) |
//! | `C` | | reset overload |
//! | `a0` / `a2` | | current range automatic / high (rev ≥ 5) |
//! | `p0` `p1` `p2`, `r#####` | | periodic LIN fault mode, period 200000/f (rev ≥ 4) |
//! | `x0` / `x1` | | CAN capacitors off / on (rev ≥ 4) |
//! | `z`, then `u1` | `u DDDDD RRRRR` | CAN diff voltage dominant / recessive, V·1000 (rev ≥ 4) |
//! | `X0` / `X1` | | CAN lines crossed (rev ≥ 8) |
//!
//! Every command and every reply ends in `\r`.

use serde::Serialize;

use crate::error::{Error, Result};

pub const CR: u8 = b'\r';

/// Supply terminals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Terminal {
    /// VBatt.
    Kl30,
    /// Ignition.
    Kl15,
    /// Ground.
    Kl31,
}

impl Terminal {
    pub const ALL: [Terminal; 3] = [Terminal::Kl30, Terminal::Kl15, Terminal::Kl31];

    pub fn label(self) -> &'static str {
        match self {
            Terminal::Kl30 => "KL30",
            Terminal::Kl15 => "KL15",
            Terminal::Kl31 => "KL31",
        }
    }
}

/// Which bus lines are cut between tester and ECU. CAN_H is also the LIN line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub enum LineInterrupt {
    #[default]
    None,
    CanH,
    CanL,
    Both,
}

/// What a bus line is shorted to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub enum LineShort {
    #[default]
    None,
    Gnd,
    Vbatt,
    /// CAN_H to CAN_L; only valid for CAN_H.
    CanL,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub enum LinPeriodicFault {
    #[default]
    Off,
    ToGnd,
    ToVbatt,
}

/// Supply of a LINcab plugged into the Y-cable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub enum LinCabSupply {
    /// The LINcab makes its own 12 V.
    #[default]
    Internal,
    /// External if more than 12 V are on pin 9, otherwise internal.
    ExternalOrInternal,
    /// External (pin 9) only; unsupplied without it.
    External,
    /// Accepted by the DLL, not documented in the manual.
    Mode3,
}

/// Firmware and hardware revision from the `t` handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Version {
    pub firmware: (u32, u32),
    /// Decides which features the box has, see [`Feature`].
    pub hw_revision: u32,
}

/// One `w` reading.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Measurement {
    pub kl30: f64,
    pub kl15: f64,
    /// GND' in volts; only meaningful while a ground offset is on.
    pub kl31: f64,
    pub analog_input: f64,
    /// Current at terminals 30/15 in mA.
    pub current_ma: f64,
    pub digital_input: bool,
}

impl Measurement {
    /// Power drawn from KL30 in watts.
    pub fn watts(&self) -> f64 {
        self.kl30 * self.current_ma / 1000.0
    }
}

/// One `s` reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DeviceState {
    pub overload: bool,
    /// Raw bytes; their meaning is not documented.
    pub error_flags: (u8, u8),
}

/// Features that need a minimum hardware revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    SafeState,
    TesterGroundOffset,
    PeriodicLinFault,
    CanMeasurement,
    CanCapacitors,
    CurrentRange,
    CrossCanLines,
}

impl Feature {
    pub const ALL: [Feature; 7] = [
        Feature::SafeState,
        Feature::TesterGroundOffset,
        Feature::PeriodicLinFault,
        Feature::CanMeasurement,
        Feature::CanCapacitors,
        Feature::CurrentRange,
        Feature::CrossCanLines,
    ];

    pub fn min_revision(self) -> u32 {
        match self {
            Feature::SafeState | Feature::TesterGroundOffset => 2,
            Feature::PeriodicLinFault | Feature::CanMeasurement | Feature::CanCapacitors => 4,
            Feature::CurrentRange => 5,
            Feature::CrossCanLines => 8,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Feature::SafeState => "safe state",
            Feature::TesterGroundOffset => "tester ground offset",
            Feature::PeriodicLinFault => "periodic LIN fault",
            Feature::CanMeasurement => "CAN measurement",
            Feature::CanCapacitors => "CAN capacitors",
            Feature::CurrentRange => "current range",
            Feature::CrossCanLines => "crossing CAN lines",
        }
    }
}

// ---------------------------------------------------------------------------
// Encoding
// ---------------------------------------------------------------------------

pub fn relay(terminal: Terminal, closed: bool) -> &'static str {
    match (terminal, closed) {
        (Terminal::Kl30, false) => "R",
        (Terminal::Kl30, true) => "S",
        (Terminal::Kl15, false) => "T",
        (Terminal::Kl15, true) => "U",
        (Terminal::Kl31, false) => "V",
        (Terminal::Kl31, true) => "W",
    }
}

/// `letter` followed by `volts` in hundredths, four digits.
pub fn value(letter: char, volts: f64, min: f64, max: f64, what: &'static str) -> Result<String> {
    if !(min..=max).contains(&volts) || volts.is_nan() {
        return Err(Error::OutOfRange {
            what,
            value: volts,
            min,
            max,
        });
    }
    Ok(format!("{letter}{:04}", (volts * 100.0) as u32))
}

pub fn bus_faults(interrupt: LineInterrupt, can_h: LineShort, can_l: LineShort) -> Result<String> {
    let p = match interrupt {
        LineInterrupt::None => 0,
        LineInterrupt::CanH => 1,
        LineInterrupt::CanL => 2,
        LineInterrupt::Both => 3,
    };
    let short = |s: LineShort| match s {
        LineShort::None => 0,
        LineShort::Gnd => 1,
        LineShort::Vbatt => 2,
        LineShort::CanL => 3,
    };
    if can_l == LineShort::CanL {
        return Err(Error::Invalid(
            "CAN_L cannot be shorted to itself; short CAN_H to CAN_L instead".into(),
        ));
    }
    Ok(if can_h == LineShort::CanL {
        format!("c{p}80")
    } else {
        format!("c{p}{}{}", short(can_h), short(can_l))
    })
}

pub fn lin_cab(mode: LinCabSupply) -> &'static str {
    match mode {
        LinCabSupply::Internal => "L",
        LinCabSupply::ExternalOrInternal => "M",
        LinCabSupply::External => "N",
        LinCabSupply::Mode3 => "O",
    }
}

pub fn periodic_mode(mode: LinPeriodicFault) -> &'static str {
    match mode {
        LinPeriodicFault::Off => "p0",
        LinPeriodicFault::ToGnd => "p1",
        LinPeriodicFault::ToVbatt => "p2",
    }
}

/// `r` + the period as 200000/f in five digits; `None` outside 2 Hz…50 kHz.
pub fn periodic_frequency(hz: f64) -> Option<String> {
    // The DLL nudges exactly 2 Hz up: 200000/2 would not fit five digits.
    let hz = if hz == 2.0 { 2.0001 } else { hz };
    (2.0001..=50_000.0)
        .contains(&hz)
        .then(|| format!("r{:05}", (200_000.0 / hz) as u32))
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// `atol` on a fixed-width ASCII field, as the DLL does (non-digits end the number).
pub fn digits(raw: &[u8]) -> u32 {
    raw.iter()
        .take_while(|b| b.is_ascii_digit())
        .fold(0, |n, b| n * 10 + u32::from(b - b'0'))
}

pub fn parse_version(r: &[u8]) -> Version {
    let d = |i: usize| digits(&r[i..i + 1]);
    Version {
        firmware: (d(1), d(2)),
        hw_revision: d(3) * 10 + d(4),
    }
}

pub fn parse_measurement(r: &[u8]) -> Measurement {
    Measurement {
        kl30: f64::from(digits(&r[1..5])) / 100.0,
        kl15: f64::from(digits(&r[5..9])) / 100.0,
        kl31: f64::from(digits(&r[9..13])) / 100.0,
        analog_input: f64::from(digits(&r[13..17])) / 100.0,
        current_ma: f64::from(digits(&r[17..22])) / 10.0,
        digital_input: digits(&r[22..23]) == 1,
    }
}

pub fn parse_state(r: &[u8]) -> DeviceState {
    DeviceState {
        overload: digits(&r[1..2]) == 1,
        error_flags: (r[2], r[3]),
    }
}

/// (recessive, dominant) in volts from a `u` reply.
pub fn parse_diff(r: &[u8]) -> (f64, f64) {
    let dominant = f64::from(digits(&r[1..6])) / 1000.0;
    let recessive = f64::from(digits(&r[6..11])) / 1000.0;
    (recessive, dominant)
}

/// Reply lengths, including the tag and the trailing CR.
pub const VERSION_LEN: usize = 6;
pub const MEASUREMENT_LEN: usize = 24;
pub const STATE_LEN: usize = 5;
pub const DIFF_LEN: usize = 12;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_commands() {
        assert_eq!(relay(Terminal::Kl15, true), "U");
        assert_eq!(value('v', 13.5, 0.0, 28.0, "VBatt").unwrap(), "v1350");
        assert_eq!(value('v', 4.0, 0.0, 28.0, "VBatt").unwrap(), "v0400");
        assert!(value('v', 28.5, 0.0, 28.0, "VBatt").is_err());
        assert_eq!(
            bus_faults(LineInterrupt::None, LineShort::None, LineShort::None).unwrap(),
            "c000"
        );
        assert_eq!(
            bus_faults(LineInterrupt::Both, LineShort::Vbatt, LineShort::Gnd).unwrap(),
            "c321"
        );
        assert_eq!(
            bus_faults(LineInterrupt::CanH, LineShort::CanL, LineShort::Gnd).unwrap(),
            "c180"
        );
        assert!(bus_faults(LineInterrupt::None, LineShort::None, LineShort::CanL).is_err());
        assert_eq!(lin_cab(LinCabSupply::Mode3), "O");
        assert_eq!(periodic_frequency(1000.0).as_deref(), Some("r00200"));
        assert_eq!(periodic_frequency(2.0).as_deref(), Some("r99995"));
        assert_eq!(periodic_frequency(60_000.0), None);
    }

    #[test]
    fn parses_replies() {
        assert_eq!(
            parse_version(b"t2308\r"),
            Version {
                firmware: (2, 3),
                hw_revision: 8
            }
        );
        let m = parse_measurement(b"w135013490000051201234\x31\r");
        assert_eq!(m.kl30, 13.5);
        assert_eq!(m.kl15, 13.49);
        assert_eq!(m.analog_input, 5.12);
        assert!((m.current_ma - 123.4).abs() < 1e-9);
        assert!(m.digital_input);
        assert_eq!(
            parse_state(b"s1\x02\x00\r"),
            DeviceState {
                overload: true,
                error_flags: (2, 0)
            }
        );
        assert_eq!(parse_diff(b"u0201000012\r"), (0.012, 2.01));
        assert_eq!(digits(b"12a4"), 12);
    }
}
