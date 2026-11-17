//! The device thread: owns the [`Vh1160`], polls it and runs commands.
//!
//! Serial I/O blocks (replies take milliseconds, a CAN measurement takes a
//! second), so it never runs on the UI thread. Commands arrive over a std
//! channel; readings and results go back over an async channel the UI awaits.

use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use vh1160_core::{
    DeviceState, LinCabSupply, LinPeriodicFault, LineInterrupt, LineShort, Measurement, SimHandle,
    Terminal, Version, Vh1160,
};

pub const POLL: Duration = Duration::from_millis(250);
/// The overload state is read every this many measurements.
const STATE_EVERY: u32 = 4;
/// Disconnect after this many failed polls in a row.
const MAX_FAILURES: u32 = 3;

#[derive(Clone, Debug)]
pub enum Target {
    Port(String),
    Sim,
}

#[derive(Debug)]
pub enum Cmd {
    Connect(Target),
    Disconnect,
    Vbatt(f64),
    Relay(Terminal, bool),
    PowerOn(f64),
    PowerOff,
    ResetEcu(Duration),
    Safe,
    ResetOverload,
    Faults(LineInterrupt, LineShort, LineShort),
    Periodic(LinPeriodicFault, f64),
    GroundOffset(f64),
    TesterOffset(f64),
    LinLimit(f64),
    Cab(LinCabSupply),
    AnalogOut(f64),
    DigitalOut(bool),
    CurrentRange { auto: bool },
    Capacitors(bool),
    CrossLines(bool),
    Termination,
    Diff(Duration),
    SimLoad(f64),
    SimOverload,
}

#[derive(Debug)]
pub enum Event {
    Connected {
        version: Version,
        label: String,
        sim: bool,
    },
    Disconnected {
        error: Option<String>,
    },
    Sample {
        t: f64,
        m: Measurement,
    },
    State(DeviceState),
    Termination(f64, f64),
    Diff(f64, f64),
    /// A command failed; the connection is still up.
    Error(String),
}

pub struct Worker {
    tx: mpsc::Sender<Cmd>,
}

impl Worker {
    pub fn spawn() -> (Self, UnboundedReceiver<Event>) {
        let (tx, rx) = mpsc::channel();
        let (etx, erx) = unbounded();
        std::thread::Builder::new()
            .name("vh1160-device".into())
            .spawn(move || run(rx, etx))
            .expect("spawn device thread");
        (Self { tx }, erx)
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }
}

struct Conn {
    vh: Vh1160,
    sim: Option<SimHandle>,
}

fn run(rx: mpsc::Receiver<Cmd>, tx: UnboundedSender<Event>) {
    let start = Instant::now();
    let mut conn: Option<Conn> = None;
    let mut next_poll = Instant::now();
    let mut polls = 0u32;
    let mut failures = 0u32;
    let emit = |e: Event| {
        let _ = tx.unbounded_send(e);
    };

    loop {
        let wait = next_poll.saturating_duration_since(Instant::now());
        match rx.recv_timeout(if conn.is_some() {
            wait
        } else {
            Duration::from_secs(3600)
        }) {
            Ok(Cmd::Connect(target)) => {
                conn = None;
                let opened = match &target {
                    Target::Port(p) => Vh1160::open(p)
                        .map(|vh| (vh, None))
                        .map_err(|e| format!("{p}: {e}")),
                    Target::Sim => Ok({
                        let (vh, h) = Vh1160::simulated();
                        (vh, Some(h))
                    }),
                };
                match opened {
                    Ok((vh, sim)) => {
                        let label = match &target {
                            Target::Port(p) => p.clone(),
                            Target::Sim => "Simulator".into(),
                        };
                        emit(Event::Connected {
                            version: vh.version(),
                            label,
                            sim: sim.is_some(),
                        });
                        conn = Some(Conn { vh, sim });
                        next_poll = Instant::now();
                        polls = 0;
                        failures = 0;
                    }
                    Err(e) => emit(Event::Disconnected { error: Some(e) }),
                }
            }
            Ok(Cmd::Disconnect) => {
                conn = None;
                emit(Event::Disconnected { error: None });
            }
            Ok(cmd) => {
                let Some(c) = conn.as_mut() else { continue };
                if let Err(e) = execute(c, cmd, &emit) {
                    emit(Event::Error(e.to_string()));
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }

        let Some(c) = conn.as_mut() else { continue };
        if Instant::now() < next_poll {
            continue;
        }
        next_poll += POLL;
        if next_poll < Instant::now() {
            next_poll = Instant::now() + POLL;
        }
        let result = c.vh.measure().and_then(|m| {
            emit(Event::Sample {
                t: start.elapsed().as_secs_f64(),
                m,
            });
            if polls.is_multiple_of(STATE_EVERY) {
                emit(Event::State(c.vh.state()?));
            }
            Ok(())
        });
        polls = polls.wrapping_add(1);
        match result {
            Ok(()) => failures = 0,
            Err(e) => {
                failures += 1;
                if failures >= MAX_FAILURES
                    || matches!(e, vh1160_core::Error::Io(_) | vh1160_core::Error::Serial(_))
                {
                    conn = None;
                    emit(Event::Disconnected {
                        error: Some(format!("connection lost: {e}")),
                    });
                }
            }
        }
    }
}

fn execute(c: &mut Conn, cmd: Cmd, emit: &impl Fn(Event)) -> vh1160_core::Result<()> {
    let vh = &mut c.vh;
    match cmd {
        Cmd::Connect(_) | Cmd::Disconnect => unreachable!(),
        Cmd::Vbatt(v) => vh.set_vbatt(v, None)?,
        Cmd::Relay(t, on) => vh.set_relay(t, on, Vh1160::RELAY_SETTLE)?,
        Cmd::PowerOn(v) => vh.power_on(Some(v))?,
        Cmd::PowerOff => vh.power_off()?,
        Cmd::ResetEcu(off) => vh.reset_ecu(off)?,
        Cmd::Safe => vh.safe_state()?,
        Cmd::ResetOverload => vh.reset_overload()?,
        Cmd::Faults(i, h, l) => vh.set_bus_faults(i, h, l)?,
        Cmd::Periodic(m, hz) => vh.set_periodic_lin_fault(m, hz)?,
        Cmd::GroundOffset(v) => vh.set_ground_offset(v)?,
        Cmd::TesterOffset(v) => vh.set_tester_ground_offset(v)?,
        Cmd::LinLimit(v) => vh.set_lin_recessive_limit(v)?,
        Cmd::Cab(m) => vh.set_lin_cab_supply(m)?,
        Cmd::AnalogOut(v) => vh.set_analog_output(v)?,
        Cmd::DigitalOut(b) => vh.set_digital_output(b)?,
        Cmd::CurrentRange { auto } => vh.set_current_range(auto)?,
        Cmd::Capacitors(b) => vh.set_can_capacitors(b)?,
        Cmd::CrossLines(b) => vh.set_can_lines_crossed(b)?,
        Cmd::Termination => {
            let (p, n) = vh.measure_can_termination()?;
            emit(Event::Termination(p, n));
        }
        Cmd::Diff(period) => {
            let (r, d) = vh.measure_can_diff_voltage(period)?;
            emit(Event::Diff(r, d));
        }
        Cmd::SimLoad(ma) => {
            if let Some(s) = &c.sim {
                s.set_load_ma(ma);
            }
        }
        Cmd::SimOverload => {
            if let Some(s) = &c.sim {
                s.trigger_overload();
            }
        }
    }
    Ok(())
}
