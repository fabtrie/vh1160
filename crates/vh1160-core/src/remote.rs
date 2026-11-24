//! Sharing one box between processes.
//!
//! Windows lets only one process open a COM port. The process that has it (the
//! desktop app) serves [`Request`]s on a local socket, and other processes (the
//! CLI) send their commands there instead of opening the port. The server runs
//! each request between its own polls, so the two never interleave on the wire.
//!
//! On Windows the socket is the named pipe `\\.\pipe\vh1160`. It rejects
//! network clients, a second server cannot take the name while the first runs,
//! and the default pipe ACL gives write access only to the creating user (and
//! administrators/SYSTEM). Elsewhere it is `vh1160.sock` in `$XDG_RUNTIME_DIR`
//! (or the temp directory).
//!
//! Wire format: one JSON [`Request`] per line, answered by one JSON [`Reply`]
//! per line.

use std::io::{self, BufRead, BufReader, Write};
use std::sync::Arc;
use std::time::Duration;

#[cfg(not(windows))]
use interprocess::local_socket::GenericFilePath;
#[cfg(windows)]
use interprocess::local_socket::GenericNamespaced;
use interprocess::local_socket::{prelude::*, ListenerOptions, Name, Stream};
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::protocol::{
    DeviceState, LinCabSupply, LinPeriodicFault, LineInterrupt, LineShort, Measurement, Terminal,
    Version,
};
use crate::Vh1160;

/// The socket the desktop app serves on.
pub const SOCKET: &str = "vh1160";

/// One operation on the box. [`Request::execute`] runs it on a [`Vh1160`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Request {
    /// What the server is connected to. Answered by the server itself with
    /// [`Reply::Hello`], never sent to the box.
    Hello,
    /// Version and device state, answered with [`Reply::Info`].
    Info,
    Measure,
    State,
    Vbatt(f64),
    Relay(Terminal, bool),
    PowerOn(Option<f64>),
    PowerOff,
    ResetEcu(Duration),
    Safe,
    ResetOverload,
    BusFaults(LineInterrupt, LineShort, LineShort),
    PeriodicLinFault(LinPeriodicFault, f64),
    GroundOffset(f64),
    TesterGroundOffset(f64),
    LinRecessiveLimit(f64),
    LinCabSupply(LinCabSupply),
    AnalogOutput(f64),
    DigitalOutput(bool),
    CurrentRange {
        auto: bool,
    },
    CanCapacitors(bool),
    CanLinesCrossed(bool),
    /// Answered with [`Reply::Pair`]: ohms with positive and negative polarity.
    CanTermination,
    /// Answered with [`Reply::Pair`]: recessive and dominant volts.
    CanDiffVoltage(Duration),
    Raw {
        command: String,
        reply_len: usize,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Reply {
    /// `None`: the server runs but has no box connected (so the port is free).
    Hello(Option<Peer>),
    Done,
    Info {
        version: Version,
        state: DeviceState,
    },
    Measurement(Measurement),
    State(DeviceState),
    Pair(f64, f64),
    Raw(Vec<u8>),
    NotConnected,
    Error(String),
}

/// What a server is connected to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Peer {
    /// The COM port, or `None` for the simulator.
    pub port: Option<String>,
    pub version: Version,
}

impl Request {
    /// Runs the request on `vh`. [`Request::Hello`] answers `Hello(None)`; a
    /// server answers it itself.
    pub fn execute(&self, vh: &mut Vh1160) -> Result<Reply> {
        use Request as R;
        Ok(match self {
            R::Hello => Reply::Hello(None),
            R::Info => Reply::Info {
                version: vh.version(),
                state: vh.state()?,
            },
            R::Measure => Reply::Measurement(vh.measure()?),
            R::State => Reply::State(vh.state()?),
            R::CanTermination => {
                let (p, n) = vh.measure_can_termination()?;
                Reply::Pair(p, n)
            }
            R::CanDiffVoltage(period) => {
                let (r, d) = vh.measure_can_diff_voltage(*period)?;
                Reply::Pair(r, d)
            }
            R::Raw { command, reply_len } => Reply::Raw(vh.raw(command, *reply_len)?),
            R::Vbatt(v) => done(vh.set_vbatt(*v, None))?,
            R::Relay(t, on) => done(vh.set_relay(*t, *on, Vh1160::RELAY_SETTLE))?,
            R::PowerOn(v) => done(vh.power_on(*v))?,
            R::PowerOff => done(vh.power_off())?,
            R::ResetEcu(off) => done(vh.reset_ecu(*off))?,
            R::Safe => done(vh.safe_state())?,
            R::ResetOverload => done(vh.reset_overload())?,
            R::BusFaults(i, h, l) => done(vh.set_bus_faults(*i, *h, *l))?,
            R::PeriodicLinFault(m, hz) => done(vh.set_periodic_lin_fault(*m, *hz))?,
            R::GroundOffset(v) => done(vh.set_ground_offset(*v))?,
            R::TesterGroundOffset(v) => done(vh.set_tester_ground_offset(*v))?,
            R::LinRecessiveLimit(v) => done(vh.set_lin_recessive_limit(*v))?,
            R::LinCabSupply(m) => done(vh.set_lin_cab_supply(*m))?,
            R::AnalogOutput(v) => done(vh.set_analog_output(*v))?,
            R::DigitalOutput(b) => done(vh.set_digital_output(*b))?,
            R::CurrentRange { auto } => done(vh.set_current_range(*auto))?,
            R::CanCapacitors(b) => done(vh.set_can_capacitors(*b))?,
            R::CanLinesCrossed(b) => done(vh.set_can_lines_crossed(*b))?,
        })
    }
}

fn done(r: Result<()>) -> Result<Reply> {
    r.map(|()| Reply::Done)
}

fn socket_name(name: &str) -> io::Result<Name<'static>> {
    #[cfg(windows)]
    {
        name.to_owned().to_ns_name::<GenericNamespaced>()
    }
    #[cfg(not(windows))]
    {
        let dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        dir.join(format!("{name}.sock"))
            .to_fs_name::<GenericFilePath>()
    }
}

/// A connection to a server.
pub struct Client {
    conn: BufReader<Stream>,
}

impl Client {
    /// Connects to the desktop app. Fails at once if it is not running.
    pub fn connect() -> io::Result<Self> {
        Self::connect_to(SOCKET)
    }

    pub fn connect_to(name: &str) -> io::Result<Self> {
        Ok(Self {
            conn: BufReader::new(Stream::connect(socket_name(name)?)?),
        })
    }

    /// Sends one request and waits for its reply (as long as the box takes).
    pub fn call(&mut self, req: &Request) -> io::Result<Reply> {
        let mut line = serde_json::to_string(req)?;
        line.push('\n');
        self.conn.get_mut().write_all(line.as_bytes())?;
        self.conn.get_mut().flush()?;
        let mut reply = String::new();
        if self.conn.read_line(&mut reply)? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the server closed the connection",
            ));
        }
        Ok(serde_json::from_str(&reply)?)
    }
}

/// Starts serving `name` on a background thread, one thread per client, until
/// the process exits. Fails if the name is taken, e.g. by another app instance.
pub fn serve<F>(name: &str, handle: F) -> io::Result<()>
where
    F: Fn(Request) -> Reply + Send + Sync + 'static,
{
    let listener = ListenerOptions::new()
        .name(socket_name(name)?)
        .create_sync()?;
    let handle = Arc::new(handle);
    std::thread::Builder::new()
        .name("vh1160-remote".into())
        .spawn(move || {
            for conn in listener.incoming().filter_map(io::Result::ok) {
                let handle = handle.clone();
                let _ = std::thread::Builder::new()
                    .name("vh1160-remote-client".into())
                    .spawn(move || {
                        let _ = serve_client(conn, &*handle);
                    });
            }
        })?;
    Ok(())
}

fn serve_client(conn: Stream, handle: &dyn Fn(Request) -> Reply) -> io::Result<()> {
    let mut conn = BufReader::new(conn);
    let mut line = String::new();
    loop {
        line.clear();
        if conn.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let reply = match serde_json::from_str(&line) {
            Ok(req) => handle(req),
            Err(e) => Reply::Error(format!("bad request: {e}")),
        };
        let mut out = serde_json::to_string(&reply)?;
        out.push('\n');
        conn.get_mut().write_all(out.as_bytes())?;
        conn.get_mut().flush()?;
    }
}
