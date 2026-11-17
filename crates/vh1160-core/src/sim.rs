//! A simulated VH1160 with an ECU on its terminals, for working without hardware.
//!
//! It parses the same command bytes as the real box and answers in the same
//! format, so it also exercises the protocol code. The ECU draws its load
//! current while KL15 is on and for a while after, then sleeps. The CAN
//! termination is a resistor, the analog and digital outputs are looped back
//! to the inputs.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::error::Result;
use crate::protocol::{digits, CR};
use crate::transport::Transport;

#[derive(Debug)]
struct State {
    version: [u8; 4],
    vbatt: f64,
    /// KL30, KL15, KL31.
    relays: [bool; 3],
    load_ma: f64,
    sleep_ma: f64,
    sleep_after: Duration,
    /// Sim time KL15 last opened while powered; `None` = asleep or never woken.
    kl15_off_at: Option<Duration>,
    ground_offset: Option<f64>,
    pending_ground_offset: f64,
    tester_offset: Option<f64>,
    pending_tester_offset: f64,
    lin_limit: Option<f64>,
    pending_lin_limit: f64,
    analog_out: f64,
    digital_out: bool,
    faults: [u8; 3],
    periodic: u8,
    periodic_period: u32,
    caps: bool,
    crossed: bool,
    cab: u8,
    current_range_auto: bool,
    termination_ohms: f64,
    overload: bool,
    noise: bool,
    rng: u32,
    realtime: bool,
    start: Instant,
    virtual_time: Duration,
    inbuf: Vec<u8>,
    out: Vec<u8>,
    log: Vec<String>,
}

impl State {
    fn now(&self) -> Duration {
        if self.realtime {
            self.start.elapsed()
        } else {
            self.virtual_time
        }
    }

    /// Uniform noise in [-amplitude, amplitude] (xorshift, no deps).
    fn jitter(&mut self, amplitude: f64) -> f64 {
        if !self.noise {
            return 0.0;
        }
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (f64::from(self.rng) / f64::from(u32::MAX) * 2.0 - 1.0) * amplitude
    }

    fn powered(&self) -> bool {
        self.relays[0] && self.relays[2] && !self.overload && self.vbatt >= 6.0
    }

    fn awake(&self) -> bool {
        self.powered()
            && (self.relays[1]
                || self
                    .kl15_off_at
                    .is_some_and(|t| self.now().saturating_sub(t) < self.sleep_after))
    }

    fn current_ma(&mut self) -> f64 {
        // The termination measurement: both lines cut and driven against each other.
        if self.faults[0] == 3 && matches!((self.faults[1], self.faults[2]), (2, 1) | (1, 2)) {
            return self.vbatt / self.termination_ohms * 1000.0;
        }
        if !self.powered() {
            return 0.0;
        }
        if self.awake() {
            self.load_ma * (1.0 + self.jitter(0.02))
        } else {
            (self.sleep_ma + self.jitter(0.03)).max(0.0)
        }
    }

    fn set_relay(&mut self, k: usize, closed: bool) {
        let was_awake = self.awake();
        self.relays[k] = closed;
        if k == 1 && !closed && was_awake {
            self.kl15_off_at = Some(self.now());
        }
        if !self.powered() {
            self.kl15_off_at = None;
        }
    }

    fn value(arg: &str) -> f64 {
        f64::from(digits(arg.as_bytes())) / 100.0
    }

    fn handle(&mut self, cmd: &str) {
        self.log.push(cmd.to_string());
        let Some(letter) = cmd.chars().next() else {
            return;
        };
        let arg = &cmd[letter.len_utf8()..];
        match letter {
            't' => {
                let v = self.version;
                self.reply(format!("t{}", String::from_utf8_lossy(&v)));
            }
            'w' => {
                let on = |b: bool, v: f64| if b { v } else { 0.0 };
                let v30 = on(self.relays[0] && !self.overload, self.vbatt);
                let v15 = on(self.relays[1] && !self.overload, self.vbatt);
                let (j1, j2) = (self.jitter(0.01), self.jitter(0.01));
                let i = self.current_ma();
                let f = |v: f64| ((v.max(0.0)) * 100.0).round() as u32;
                let gnd = self.ground_offset.unwrap_or(0.0);
                let reply = format!(
                    "w{:04}{:04}{:04}{:04}{:05}{}",
                    f(if v30 > 0.0 { v30 + j1 } else { 0.0 }).min(9999),
                    f(if v15 > 0.0 { v15 + j2 } else { 0.0 }).min(9999),
                    f(gnd),
                    f(self.analog_out),
                    ((i * 10.0).round() as u32).min(99_999),
                    u8::from(self.digital_out),
                );
                self.reply(reply);
            }
            's' => {
                let mut r = vec![b's', if self.overload { b'1' } else { b'0' }, 0, 0, CR];
                self.out.append(&mut r);
            }
            'u' if arg == "1" => {
                let dom = (2000.0 + self.jitter(15.0)).round() as u32;
                let rec = (10.0 + self.jitter(4.0)).round().max(0.0) as u32;
                self.reply(format!("u{dom:05}{rec:05}"));
            }
            'R' | 'S' => self.set_relay(0, letter == 'S'),
            'T' | 'U' => self.set_relay(1, letter == 'U'),
            'V' | 'W' => self.set_relay(2, letter == 'W'),
            'v' => {
                self.vbatt = Self::value(arg);
                if !self.powered() {
                    self.kl15_off_at = None;
                }
            }
            'i' => self.pending_ground_offset = Self::value(arg),
            'h' => {
                self.ground_offset = Some(self.pending_ground_offset);
                self.lin_limit = None;
            }
            'g' => self.ground_offset = None,
            'I' => self.pending_tester_offset = Self::value(arg),
            'H' => {
                self.tester_offset = Some(self.pending_tester_offset);
                self.ground_offset = None;
                self.lin_limit = None;
            }
            'G' => self.tester_offset = None,
            'n' => self.pending_lin_limit = Self::value(arg),
            'm' => {
                self.lin_limit = Some(self.pending_lin_limit);
                self.ground_offset = None;
                self.tester_offset = None;
            }
            'l' => self.lin_limit = None,
            'A' => self.analog_out = Self::value(arg),
            'D' | 'E' => self.digital_out = letter == 'E',
            'L' | 'M' | 'N' | 'O' => self.cab = letter as u8 - b'L',
            'c' if arg.len() == 3 => {
                let b = arg.as_bytes();
                self.faults = [b[0] - b'0', b[1] - b'0', b[2] - b'0'];
                if self.faults != [0, 0, 0] {
                    self.periodic = 0;
                }
            }
            'Z' => {
                self.relays = [false; 3];
                self.kl15_off_at = None;
                self.faults = [0; 3];
                self.periodic = 0;
                self.ground_offset = None;
                self.tester_offset = None;
                self.lin_limit = None;
            }
            'C' => self.overload = false,
            'a' => self.current_range_auto = arg != "2",
            'p' => self.periodic = digits(arg.as_bytes()) as u8,
            'r' => self.periodic_period = digits(arg.as_bytes()),
            'x' => self.caps = arg == "1",
            'X' => self.crossed = arg == "1",
            _ => {}
        }
    }

    fn reply(&mut self, body: String) {
        self.out.extend_from_slice(body.as_bytes());
        self.out.push(CR);
    }
}

/// What the simulated box is currently set to, for showing in a UI or asserting in tests.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SimSnapshot {
    pub vbatt: f64,
    pub relays: [bool; 3],
    pub awake: bool,
    pub faults: [u8; 3],
    pub overload: bool,
}

/// Shared handle for steering the simulation while the device is in use.
#[derive(Clone, Debug)]
pub struct SimHandle(Arc<Mutex<State>>);

impl SimHandle {
    /// Current the ECU draws while awake.
    pub fn set_load_ma(&self, ma: f64) {
        self.0.lock().unwrap().load_ma = ma.max(0.0);
    }

    pub fn load_ma(&self) -> f64 {
        self.0.lock().unwrap().load_ma
    }

    /// How long the ECU stays awake after KL15 opens.
    pub fn set_sleep_after(&self, d: Duration) {
        self.0.lock().unwrap().sleep_after = d;
    }

    pub fn sleep_after(&self) -> Duration {
        self.0.lock().unwrap().sleep_after
    }

    pub fn set_termination_ohms(&self, ohms: f64) {
        self.0.lock().unwrap().termination_ohms = ohms.max(1.0);
    }

    /// The box switches its outputs off, as on a short circuit.
    pub fn trigger_overload(&self) {
        let mut s = self.0.lock().unwrap();
        s.overload = true;
        s.kl15_off_at = None;
    }

    /// Advances simulated time (only without real-time mode).
    pub fn advance(&self, d: Duration) {
        self.0.lock().unwrap().virtual_time += d;
    }

    /// Every command received since the last call, without the CR.
    pub fn take_log(&self) -> Vec<String> {
        std::mem::take(&mut self.0.lock().unwrap().log)
    }

    pub fn snapshot(&self) -> SimSnapshot {
        let s = self.0.lock().unwrap();
        SimSnapshot {
            vbatt: s.vbatt,
            relays: s.relays,
            awake: s.awake(),
            faults: s.faults,
            overload: s.overload,
        }
    }
}

pub struct SimTransport(Arc<Mutex<State>>);

impl SimTransport {
    /// Real-time simulation with measurement noise: firmware 2.3, hardware
    /// revision 8, 13.5 V set, relays open, 240 mA ECU that sleeps 20 s after KL15.
    pub fn new() -> (Self, SimHandle) {
        Self::with_options(true, true, *b"2308")
    }

    /// Exact readings and simulated time that only moves on waits, for tests.
    pub fn for_tests(version: [u8; 4]) -> (Self, SimHandle) {
        Self::with_options(false, false, version)
    }

    pub fn with_options(noise: bool, realtime: bool, version: [u8; 4]) -> (Self, SimHandle) {
        let state = Arc::new(Mutex::new(State {
            version,
            vbatt: 13.5,
            relays: [false; 3],
            load_ma: 240.0,
            sleep_ma: 0.35,
            sleep_after: Duration::from_secs(20),
            kl15_off_at: None,
            ground_offset: None,
            pending_ground_offset: 0.0,
            tester_offset: None,
            pending_tester_offset: 0.0,
            lin_limit: None,
            pending_lin_limit: 0.0,
            analog_out: 0.0,
            digital_out: false,
            faults: [0; 3],
            periodic: 0,
            periodic_period: 0,
            caps: false,
            crossed: false,
            cab: 0,
            current_range_auto: true,
            termination_ohms: 120.0,
            overload: false,
            noise,
            rng: 0x9E37_79B9,
            realtime,
            start: Instant::now(),
            virtual_time: Duration::ZERO,
            inbuf: Vec::new(),
            out: Vec::new(),
            log: Vec::new(),
        }));
        (Self(state.clone()), SimHandle(state))
    }
}

impl Transport for SimTransport {
    fn clear_input(&mut self) -> Result<()> {
        self.0.lock().unwrap().out.clear();
        Ok(())
    }

    fn write_all(&mut self, data: &[u8]) -> Result<()> {
        let mut s = self.0.lock().unwrap();
        s.inbuf.extend_from_slice(data);
        while let Some(pos) = s.inbuf.iter().position(|&b| b == CR) {
            let cmd: Vec<u8> = s.inbuf.drain(..=pos).collect();
            let cmd = String::from_utf8_lossy(&cmd[..cmd.len() - 1]).into_owned();
            s.handle(&cmd);
        }
        Ok(())
    }

    fn read(&mut self, n: usize) -> Result<Vec<u8>> {
        let mut s = self.0.lock().unwrap();
        let n = n.min(s.out.len());
        Ok(s.out.drain(..n).collect())
    }

    fn sleep(&mut self, d: Duration) {
        let realtime = {
            let mut s = self.0.lock().unwrap();
            if !s.realtime {
                s.virtual_time += d;
            }
            s.realtime
        };
        if realtime {
            std::thread::sleep(d);
        }
    }
}
