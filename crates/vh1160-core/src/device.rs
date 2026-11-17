use std::time::Duration;

use serde::Serialize;

use crate::error::{Error, Result};
use crate::protocol::{
    self, DeviceState, Feature, LinCabSupply, LinPeriodicFault, LineInterrupt, LineShort,
    Measurement, Terminal, Version, CR,
};
use crate::sim::{SimHandle, SimTransport};
use crate::transport::{SerialTransport, Transport};

const MS: fn(u64) -> Duration = Duration::from_millis;

/// A connected VH1160 or VH1150.
pub struct Vh1160 {
    transport: Box<dyn Transport>,
    version: Version,
    /// Last VBatt sent, for the settle time after the next change.
    vbatt: f64,
}

impl Vh1160 {
    /// Opens a serial port (e.g. `COM7` or `/dev/ttyUSB0`) and does the version handshake.
    pub fn open(port: &str) -> Result<Self> {
        Self::with_transport(Box::new(SerialTransport::open(port)?))
    }

    /// Connects to a simulated VH1160 (firmware 2.3, hardware revision 8).
    pub fn simulated() -> (Self, SimHandle) {
        let (t, handle) = SimTransport::new();
        let vh = Self::with_transport(Box::new(t)).expect("simulator always answers");
        (vh, handle)
    }

    /// Fails if the box does not answer the version request.
    pub fn with_transport(transport: Box<dyn Transport>) -> Result<Self> {
        let mut vh = Self {
            transport,
            version: Version {
                firmware: (0, 0),
                hw_revision: 0,
            },
            vbatt: 0.0,
        };
        let reply = vh.query("t", protocol::VERSION_LEN, MS(20))?;
        vh.version = protocol::parse_version(&reply);
        Ok(vh)
    }

    pub fn version(&self) -> Version {
        self.version
    }

    pub fn has(&self, feature: Feature) -> bool {
        self.version.hw_revision >= feature.min_revision()
    }

    // -- transport -------------------------------------------------------------

    fn send(&mut self, cmd: &str) -> Result<()> {
        let mut data = cmd.as_bytes().to_vec();
        data.push(CR);
        self.transport.write_all(&data)
    }

    fn send_wait(&mut self, cmd: &str, settle: Duration) -> Result<()> {
        self.send(cmd)?;
        if !settle.is_zero() {
            self.transport.sleep(settle);
        }
        Ok(())
    }

    /// Sends `cmd` and reads a `len`-byte reply tagged with its first letter and
    /// ended by CR. Two attempts, like the DLL; stale input is dropped first.
    fn query(&mut self, cmd: &str, len: usize, wait: Duration) -> Result<Vec<u8>> {
        let tag = cmd.as_bytes()[0];
        let mut problem = String::from("no answer");
        for _ in 0..2 {
            self.transport.clear_input()?;
            self.send(cmd)?;
            if !wait.is_zero() {
                self.transport.sleep(wait);
            }
            let reply = self.transport.read(len)?;
            if reply.len() < len {
                problem = if reply.is_empty() {
                    "no answer".into()
                } else {
                    format!(
                        "short answer {:?} (expected {len} bytes)",
                        String::from_utf8_lossy(&reply)
                    )
                };
            } else if reply[0] != tag {
                problem = format!("wrong answer {:?}", String::from_utf8_lossy(&reply));
            } else if reply[len - 1] != CR {
                problem = format!(
                    "answer not ended by CR: {:?}",
                    String::from_utf8_lossy(&reply)
                );
            } else {
                return Ok(reply);
            }
        }
        Err(Error::Protocol {
            cmd: cmd.into(),
            problem,
        })
    }

    fn require(&self, feature: Feature) -> Result<()> {
        if self.has(feature) {
            Ok(())
        } else {
            Err(Error::HardwareRevision {
                what: feature.name(),
                needed: feature.min_revision(),
                actual: self.version.hw_revision,
            })
        }
    }

    /// Sends a raw command (CR is appended) and reads `reply_len` bytes if > 0.
    pub fn raw(&mut self, cmd: &str, reply_len: usize) -> Result<Vec<u8>> {
        self.transport.clear_input()?;
        self.send(cmd)?;
        if reply_len == 0 {
            return Ok(Vec::new());
        }
        self.transport.sleep(MS(20));
        self.transport.read(reply_len)
    }

    // -- general ---------------------------------------------------------------

    /// Puts the box into its defined safe state (relays open).
    pub fn safe_state(&mut self) -> Result<()> {
        self.require(Feature::SafeState)?;
        self.send("Z")
    }

    pub fn state(&mut self) -> Result<DeviceState> {
        let r = self.query("s", protocol::STATE_LEN, MS(20))?;
        Ok(protocol::parse_state(&r))
    }

    /// Leaves the overload state (it comes back if the overload is still there).
    pub fn reset_overload(&mut self) -> Result<()> {
        self.send("C")
    }

    /// The DLL's close option: safe state, or below revision 2 all relays open and VBatt 0.
    pub fn shut_down(&mut self) -> Result<()> {
        if self.has(Feature::SafeState) {
            return self.safe_state();
        }
        for t in Terminal::ALL {
            self.set_relay(t, false, Duration::ZERO)?;
        }
        self.set_vbatt(0.0, Some(Duration::ZERO))
    }

    // -- terminals -------------------------------------------------------------

    /// Closes or opens a terminal relay, then waits `settle` (the DLL waits 10 ms).
    /// KL31 cannot be switched while a ground offset is on.
    pub fn set_relay(&mut self, terminal: Terminal, closed: bool, settle: Duration) -> Result<()> {
        self.send_wait(protocol::relay(terminal, closed), settle)
    }

    pub const RELAY_SETTLE: Duration = Duration::from_millis(10);
    pub const VBATT_MIN: f64 = 0.0;
    pub const VBATT_MAX: f64 = 28.0;

    /// Sets the ECU supply voltage, 0…28 V (the box regulates 2…28 V).
    /// Without `settle` it waits like the DLL: 2 ms when rising, up to 7 ms for a big drop.
    pub fn set_vbatt(&mut self, volts: f64, settle: Option<Duration>) -> Result<()> {
        let cmd = protocol::value('v', volts, Self::VBATT_MIN, Self::VBATT_MAX, "VBatt")?;
        let settle = settle.unwrap_or_else(|| {
            let drop = self.vbatt - volts;
            MS(if drop <= 1.428_571_428_571_428_6 {
                2
            } else if drop <= 2.5 {
                3
            } else if drop <= 3.571_428_571_428_571_6 {
                5
            } else {
                7
            })
        });
        self.vbatt = volts;
        self.send_wait(&cmd, settle)
    }

    /// Voltages, current and inputs in one reading.
    pub fn measure(&mut self) -> Result<Measurement> {
        self.transport.sleep(MS(11));
        let r = self.query("w", protocol::MEASUREMENT_LEN, Duration::ZERO)?;
        Ok(protocol::parse_measurement(&r))
    }

    /// Automatic range (default) or fixed high range.
    pub fn set_current_range(&mut self, auto: bool) -> Result<()> {
        self.require(Feature::CurrentRange)?;
        self.send(if auto { "a0" } else { "a2" })
    }

    // -- ground offset / LIN level ---------------------------------------------

    fn offset(
        &mut self,
        on: char,
        enable: &str,
        off: &str,
        volts: f64,
        max: f64,
        what: &'static str,
    ) -> Result<()> {
        if volts > 0.0 {
            let cmd = protocol::value(on, volts, 0.0, max, what)?;
            self.send_wait(&cmd, MS(2))?;
            self.send(enable)
        } else {
            self.send(off)
        }
    }

    /// Shifts the ECU ground by 0…10 V; 0 turns it off. Turns off the LIN recessive limit.
    pub fn set_ground_offset(&mut self, volts: f64) -> Result<()> {
        self.offset('i', "h", "g", volts, 10.0, "ground offset")
    }

    /// Shifts the tester-side bus ground by 0…20 V (LIN); 0 turns it off.
    pub fn set_tester_ground_offset(&mut self, volts: f64) -> Result<()> {
        self.require(Feature::TesterGroundOffset)?;
        self.offset('I', "H", "G", volts, 20.0, "tester ground offset")
    }

    /// Limits the LIN recessive level to 0…28 V; 0 turns it off.
    pub fn set_lin_recessive_limit(&mut self, volts: f64) -> Result<()> {
        self.offset('n', "m", "l", volts, 28.0, "LIN recessive limit")
    }

    pub fn set_lin_cab_supply(&mut self, mode: LinCabSupply) -> Result<()> {
        self.send(protocol::lin_cab(mode))
    }

    // -- I/O -------------------------------------------------------------------

    /// Analog output 0…15 V; 0 turns it off.
    pub fn set_analog_output(&mut self, volts: f64) -> Result<()> {
        let cmd = protocol::value('A', volts, 0.0, 15.0, "analog output")?;
        self.send(&cmd)
    }

    pub fn set_digital_output(&mut self, high: bool) -> Result<()> {
        self.send(if high { "E" } else { "D" })
    }

    // -- bus faults ------------------------------------------------------------

    /// Cuts lines and/or shorts them to GND/VBatt. CAN_H is the LIN line. All `None` = normal.
    pub fn set_bus_faults(
        &mut self,
        interrupt: LineInterrupt,
        can_h: LineShort,
        can_l: LineShort,
    ) -> Result<()> {
        let cmd = protocol::bus_faults(interrupt, can_h, can_l)?;
        self.send(&cmd)
    }

    pub fn clear_bus_faults(&mut self) -> Result<()> {
        self.set_bus_faults(LineInterrupt::None, LineShort::None, LineShort::None)
    }

    /// Shorts the LIN line periodically (2 Hz…50 kHz) to GND or VBatt; `Off` stops it.
    pub fn set_periodic_lin_fault(&mut self, mode: LinPeriodicFault, hz: f64) -> Result<()> {
        self.require(Feature::PeriodicLinFault)?;
        let freq = protocol::periodic_frequency(hz);
        if freq.is_none() && mode != LinPeriodicFault::Off {
            return Err(Error::OutOfRange {
                what: "frequency",
                value: hz,
                min: 2.0,
                max: 50_000.0,
            });
        }
        self.send_wait(protocol::periodic_mode(mode), MS(2))?;
        if let Some(f) = freq {
            self.send(&f)?;
        }
        Ok(())
    }

    // -- CAN -------------------------------------------------------------------

    /// Termination resistance in ohms, with positive and negative polarity.
    ///
    /// The DLL's sequence: safe state, VBatt 4 V, both lines cut from the ECU and
    /// driven to VBatt/GND (then swapped), R = 4 V / current. Ends in safe state,
    /// so the ECU loses power.
    pub fn measure_can_termination(&mut self) -> Result<(f64, f64)> {
        self.require(Feature::CanMeasurement)?;
        self.safe_state()?;
        self.transport.sleep(MS(50));
        self.set_vbatt(4.0, Some(Duration::ZERO))?;
        self.transport.sleep(MS(50));
        let mut out = [0.0; 2];
        for (k, (h, l)) in [
            (LineShort::Vbatt, LineShort::Gnd),
            (LineShort::Gnd, LineShort::Vbatt),
        ]
        .into_iter()
        .enumerate()
        {
            self.set_bus_faults(LineInterrupt::Both, h, l)?;
            self.transport.sleep(MS(50));
            let ma = self.measure()?.current_ma;
            out[k] = 4.0 / if ma > 0.0 { ma } else { 1e-5 } * 1000.0;
            self.transport.sleep(MS(50));
        }
        self.safe_state()?;
        Ok((out[0], out[1]))
    }

    /// (recessive, dominant) differential CAN voltage in volts, measured over `period`.
    pub fn measure_can_diff_voltage(&mut self, period: Duration) -> Result<(f64, f64)> {
        self.require(Feature::CanMeasurement)?;
        let period = if period.is_zero() { MS(1000) } else { period };
        self.send_wait("z", period)?;
        self.transport.sleep(MS(50));
        let r = self.query("u1", protocol::DIFF_LEN, Duration::ZERO)?;
        Ok(protocol::parse_diff(&r))
    }

    /// Switches 1.6 nF to GND onto CAN_H and CAN_L.
    pub fn set_can_capacitors(&mut self, on: bool) -> Result<()> {
        self.require(Feature::CanCapacitors)?;
        self.send(if on { "x1" } else { "x0" })
    }

    /// Swaps CAN_H and CAN_L.
    pub fn set_can_lines_crossed(&mut self, crossed: bool) -> Result<()> {
        self.require(Feature::CrossCanLines)?;
        self.send(if crossed { "X1" } else { "X0" })
    }

    // -- recipes ---------------------------------------------------------------

    /// Optionally sets VBatt, then closes KL31, KL30 and KL15.
    pub fn power_on(&mut self, volts: Option<f64>) -> Result<()> {
        if let Some(v) = volts {
            self.set_vbatt(v, None)?;
        }
        for t in [Terminal::Kl31, Terminal::Kl30, Terminal::Kl15] {
            self.set_relay(t, true, Self::RELAY_SETTLE)?;
        }
        Ok(())
    }

    /// Opens KL15, then KL30 (ground stays connected).
    pub fn power_off(&mut self) -> Result<()> {
        self.set_relay(Terminal::Kl15, false, Self::RELAY_SETTLE)?;
        self.set_relay(Terminal::Kl30, false, Self::RELAY_SETTLE)
    }

    /// Hardware reset: power off, wait `off`, power on.
    pub fn reset_ecu(&mut self, off: Duration) -> Result<()> {
        self.power_off()?;
        self.transport.sleep(off);
        self.power_on(None)
    }
}

/// A serial port, as listed by the OS.
#[derive(Debug, Clone, Serialize)]
pub struct PortEntry {
    pub name: String,
    pub description: String,
    /// FTDI USB serial device (the VH1160 is one).
    pub ftdi: bool,
}

const FTDI_VID: u16 = 0x0403;

pub fn list_ports() -> Result<Vec<PortEntry>> {
    let mut ports: Vec<PortEntry> = serialport::available_ports()?
        .into_iter()
        .map(|p| {
            let (description, ftdi) = match &p.port_type {
                serialport::SerialPortType::UsbPort(u) => (
                    u.product
                        .clone()
                        .unwrap_or_else(|| "USB serial port".into()),
                    u.vid == FTDI_VID,
                ),
                serialport::SerialPortType::PciPort => ("PCI serial port".into(), false),
                serialport::SerialPortType::BluetoothPort => {
                    ("Bluetooth serial port".into(), false)
                }
                serialport::SerialPortType::Unknown => ("serial port".into(), false),
            };
            PortEntry {
                name: p.port_name,
                description,
                ftdi,
            }
        })
        .collect();
    ports.sort_by_key(|p| port_sort_key(&p.name));
    Ok(ports)
}

fn port_sort_key(name: &str) -> (String, u32) {
    let digits: String = name
        .chars()
        .rev()
        .take_while(char::is_ascii_digit)
        .collect::<String>();
    let num = digits
        .chars()
        .rev()
        .collect::<String>()
        .parse()
        .unwrap_or(0);
    (
        name.trim_end_matches(|c: char| c.is_ascii_digit())
            .to_string(),
        num,
    )
}

/// FTDI ports that answer the version request, with their version.
/// Sends `t\r` to every FTDI port, other FTDI devices included.
pub fn find_devices() -> Result<Vec<(String, Version)>> {
    let mut found = Vec::new();
    for p in list_ports()?.into_iter().filter(|p| p.ftdi) {
        let opened = SerialTransport::open_with_timeout(&p.name, MS(200))
            .and_then(|t| Vh1160::with_transport(Box::new(t)));
        if let Ok(vh) = opened {
            found.push((p.name, vh.version()));
        }
    }
    Ok(found)
}
