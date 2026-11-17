use std::fs::File;
use std::io::{BufWriter, Write};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use clap::{Args, Parser, Subcommand, ValueEnum};
use vh1160_core::protocol::Feature;
use vh1160_core::{
    find_devices, list_ports, LinCabSupply, LinPeriodicFault, LineInterrupt, LineShort, Terminal,
    Vh1160,
};

#[derive(Parser)]
#[command(
    name = "vh1160",
    version,
    about = "Control a Vector VH1160 / VH1150 ECU test box over USB"
)]
struct Cli {
    /// Serial port, e.g. COM7. Auto-detected if omitted.
    #[arg(short, long, global = true, conflicts_with = "sim")]
    port: Option<String>,

    /// Use the built-in simulator instead of real hardware.
    #[arg(long, global = true)]
    sim: bool,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List serial ports and which ones answer as a VH1150/VH1160.
    Ports,
    /// Firmware, hardware revision, features and overload state.
    Info,
    /// Take one measurement (voltages, current, inputs).
    Read {
        /// Print JSON instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Stream measurements until Ctrl+C.
    Monitor {
        /// Time between readings, e.g. 250ms, 1s.
        #[arg(long, default_value = "250ms", value_parser = parse_duration)]
        interval: Duration,
        /// Also write readings to this CSV file.
        #[arg(long)]
        csv: Option<String>,
        /// Stop after this many readings.
        #[arg(long)]
        count: Option<u64>,
    },
    /// Set the supply voltage (0 to 28 V).
    Vbatt { volts: f64 },
    /// Switch a terminal relay.
    Relay {
        #[arg(value_enum)]
        terminal: TerminalArg,
        #[arg(value_enum)]
        state: OnOff,
    },
    /// ECU on: close KL31, KL30 and KL15 (optionally set VBatt first).
    On { volts: Option<f64> },
    /// ECU off: open KL15 and KL30.
    Off,
    /// Power-cycle the ECU.
    Reset {
        /// How long the ECU stays off, e.g. 1s, 500ms.
        #[arg(long, default_value = "1s", value_parser = parse_duration)]
        off: Duration,
    },
    /// Put the box into its safe state (all relays open).
    Safe,
    /// Leave the overload state.
    ResetOverload,
    /// CAN/LIN line faults.
    #[command(args_conflicts_with_subcommands = true)]
    Fault {
        #[command(subcommand)]
        sub: Option<FaultCmd>,
        #[command(flatten)]
        set: FaultArgs,
    },
    /// LIN levels and LINcab supply.
    Lin {
        #[command(subcommand)]
        sub: LinCmd,
    },
    /// CAN measurements and line options.
    Can {
        #[command(subcommand)]
        sub: CanCmd,
    },
    /// Analog/digital I/O and the current range.
    Io {
        #[command(subcommand)]
        sub: IoCmd,
    },
    /// Send a raw command (CR is appended) and print the reply.
    Raw {
        command: String,
        /// Number of reply bytes to read (0 = no reply expected).
        #[arg(default_value_t = 0)]
        reply_len: usize,
    },
}

#[derive(Args)]
struct FaultArgs {
    /// Lines to cut between tester and ECU.
    #[arg(long, value_enum)]
    interrupt: Option<InterruptArg>,
    /// Short CAN_H / LIN to GND, VBatt or CAN_L.
    #[arg(long, value_enum)]
    short_h: Option<ShortHArg>,
    /// Short CAN_L to GND or VBatt.
    #[arg(long, value_enum)]
    short_l: Option<ShortLArg>,
}

#[derive(Subcommand)]
enum FaultCmd {
    /// Remove all line faults.
    Clear,
    /// Short the LIN line periodically.
    Periodic {
        #[arg(value_enum)]
        mode: PeriodicArg,
        /// Frequency, 2 Hz to 50 kHz.
        #[arg(long, default_value_t = 1000.0)]
        hz: f64,
    },
}

#[derive(Subcommand)]
enum LinCmd {
    /// Shift the ECU ground (0 to 10 V; 0 = off).
    GroundOffset { volts: f64 },
    /// Shift the tester-side bus ground (0 to 20 V; 0 = off).
    TesterOffset { volts: f64 },
    /// Limit the LIN recessive level (0 to 28 V; 0 = off).
    RecessiveLimit { volts: f64 },
    /// LINcab supply mode.
    Cab {
        #[arg(value_enum)]
        mode: CabArg,
    },
}

#[derive(Subcommand)]
enum CanCmd {
    /// Measure the termination resistor (ends in safe state: the ECU loses power).
    Termination,
    /// Measure the recessive and dominant differential voltage.
    Diff {
        #[arg(long, default_value = "1s", value_parser = parse_duration)]
        period: Duration,
    },
    /// 1.6 nF to GND on CAN_H and CAN_L.
    Caps {
        #[arg(value_enum)]
        state: OnOff,
    },
    /// Swap CAN_H and CAN_L.
    Cross {
        #[arg(value_enum)]
        state: OnOff,
    },
}

#[derive(Subcommand)]
enum IoCmd {
    /// Set the analog output (0 to 15 V; 0 = off).
    Aout { volts: f64 },
    /// Set the digital output.
    Dout {
        #[arg(value_enum)]
        level: Level,
    },
    /// Current measurement range.
    Range {
        #[arg(value_enum)]
        range: RangeArg,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum TerminalArg {
    #[value(name = "30")]
    Kl30,
    #[value(name = "15")]
    Kl15,
    #[value(name = "31")]
    Kl31,
}

#[derive(Clone, Copy, PartialEq, ValueEnum)]
enum OnOff {
    On,
    Off,
}

#[derive(Clone, Copy, ValueEnum)]
enum Level {
    High,
    Low,
}

#[derive(Clone, Copy, ValueEnum)]
enum InterruptArg {
    None,
    H,
    L,
    Both,
}

#[derive(Clone, Copy, ValueEnum)]
enum ShortHArg {
    None,
    Gnd,
    Vbatt,
    Canl,
}

#[derive(Clone, Copy, ValueEnum)]
enum ShortLArg {
    None,
    Gnd,
    Vbatt,
}

#[derive(Clone, Copy, ValueEnum)]
enum PeriodicArg {
    Off,
    Gnd,
    Vbatt,
}

#[derive(Clone, Copy, ValueEnum)]
enum CabArg {
    Internal,
    Auto,
    External,
    Mode3,
}

#[derive(Clone, Copy, ValueEnum)]
enum RangeArg {
    Auto,
    High,
}

fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    let (num, scale) = if let Some(n) = s.strip_suffix("ms") {
        (n, 0.001)
    } else if let Some(n) = s.strip_suffix('s') {
        (n, 1.0)
    } else {
        (s, 0.001)
    };
    let value: f64 = num
        .trim()
        .parse()
        .map_err(|_| format!("invalid duration {s:?}"))?;
    if value <= 0.0 {
        return Err("duration must be positive".into());
    }
    Ok(Duration::from_secs_f64(value * scale))
}

fn connect(cli: &Cli) -> Result<Vh1160, String> {
    if cli.sim {
        return Ok(Vh1160::simulated().0);
    }
    let port = match &cli.port {
        Some(p) => p.clone(),
        None => {
            let found = find_devices().map_err(|e| e.to_string())?;
            match found.as_slice() {
                [] => return Err("no VH1160 found; pass --port or try `vh1160 ports`".into()),
                [(port, _)] => port.clone(),
                many => {
                    let names: Vec<_> = many.iter().map(|(p, _)| p.as_str()).collect();
                    return Err(format!(
                        "several VH1160 found: {}; pick one with --port",
                        names.join(", ")
                    ));
                }
            }
        }
    };
    Vh1160::open(&port).map_err(|e| format!("{port}: {e}"))
}

fn escape(bytes: &[u8]) -> String {
    bytes
        .iter()
        .flat_map(|&b| std::ascii::escape_default(b))
        .map(char::from)
        .collect()
}

fn run(cli: Cli) -> Result<(), String> {
    if let Cmd::Ports = cli.cmd {
        let ports = list_ports().map_err(|e| e.to_string())?;
        if ports.is_empty() {
            println!("No serial ports found.");
            return Ok(());
        }
        let devices = find_devices().map_err(|e| e.to_string())?;
        for p in ports {
            let found = devices
                .iter()
                .find(|(name, _)| *name == p.name)
                .map(|(_, v)| {
                    format!(
                        "  <- VH1160  FW {}.{}  HW rev {}",
                        v.firmware.0, v.firmware.1, v.hw_revision
                    )
                })
                .unwrap_or_default();
            println!(
                "{:<8} {:<5} {}{}",
                p.name,
                if p.ftdi { "FTDI" } else { "" },
                p.description,
                found
            );
        }
        return Ok(());
    }

    let mut vh = connect(&cli)?;
    let e = |e: vh1160_core::Error| e.to_string();

    match cli.cmd {
        Cmd::Ports => unreachable!(),
        Cmd::Info => {
            let v = vh.version();
            let st = vh.state().map_err(e)?;
            let features: Vec<_> = Feature::ALL
                .iter()
                .filter(|f| vh.has(**f))
                .map(|f| f.name())
                .collect();
            println!("firmware     {}.{}", v.firmware.0, v.firmware.1);
            println!("hw revision  {}", v.hw_revision);
            println!("overload     {}", if st.overload { "YES" } else { "no" });
            println!(
                "error flags  {:#04x} {:#04x}",
                st.error_flags.0, st.error_flags.1
            );
            println!(
                "features     {}",
                if features.is_empty() {
                    "basic".into()
                } else {
                    features.join(", ")
                }
            );
        }
        Cmd::Read { json } => {
            let m = vh.measure().map_err(e)?;
            if json {
                println!("{}", serde_json::to_string(&m).unwrap());
            } else {
                println!("KL30        {:>7.2} V", m.kl30);
                println!("KL15        {:>7.2} V", m.kl15);
                println!("current     {:>7.1} mA", m.current_ma);
                println!("power       {:>7.2} W", m.watts());
                println!("analog in   {:>7.2} V", m.analog_input);
                println!("digital in  {:>7}", u8::from(m.digital_input));
            }
        }
        Cmd::Monitor {
            interval,
            csv,
            count,
        } => {
            let mut out = match csv {
                Some(path) => {
                    let mut w = BufWriter::new(
                        File::create(&path).map_err(|err| format!("{path}: {err}"))?,
                    );
                    writeln!(w, "time_s,kl30_v,kl15_v,current_ma,analog_in_v,digital_in")
                        .map_err(|err| err.to_string())?;
                    Some(w)
                }
                None => None,
            };
            println!(
                "{:>8}  {:>7}  {:>7}  {:>10}",
                "time", "KL30 V", "KL15 V", "current mA"
            );
            let start = Instant::now();
            let mut n = 0u64;
            while count.is_none_or(|c| n < c) {
                let next = start + interval * (n as u32 + 1);
                let m = vh.measure().map_err(e)?;
                let t = start.elapsed().as_secs_f64();
                println!(
                    "{t:>7.2}s  {:>7.2}  {:>7.2}  {:>10.1}",
                    m.kl30, m.kl15, m.current_ma
                );
                if let Some(w) = out.as_mut() {
                    writeln!(
                        w,
                        "{t:.3},{:.2},{:.2},{:.1},{:.2},{}",
                        m.kl30,
                        m.kl15,
                        m.current_ma,
                        m.analog_input,
                        u8::from(m.digital_input)
                    )
                    .and_then(|_| w.flush())
                    .map_err(|err| err.to_string())?;
                }
                n += 1;
                if let Some(wait) = next.checked_duration_since(Instant::now()) {
                    std::thread::sleep(wait);
                }
            }
        }
        Cmd::Vbatt { volts } => vh.set_vbatt(volts, None).map_err(e)?,
        Cmd::Relay { terminal, state } => {
            let t = match terminal {
                TerminalArg::Kl30 => Terminal::Kl30,
                TerminalArg::Kl15 => Terminal::Kl15,
                TerminalArg::Kl31 => Terminal::Kl31,
            };
            vh.set_relay(t, state == OnOff::On, Vh1160::RELAY_SETTLE)
                .map_err(e)?;
        }
        Cmd::On { volts } => vh.power_on(volts).map_err(e)?,
        Cmd::Off => vh.power_off().map_err(e)?,
        Cmd::Reset { off } => vh.reset_ecu(off).map_err(e)?,
        Cmd::Safe => vh.safe_state().map_err(e)?,
        Cmd::ResetOverload => vh.reset_overload().map_err(e)?,
        Cmd::Fault {
            sub: Some(FaultCmd::Clear),
            ..
        } => vh.clear_bus_faults().map_err(e)?,
        Cmd::Fault {
            sub: Some(FaultCmd::Periodic { mode, hz }),
            ..
        } => {
            let mode = match mode {
                PeriodicArg::Off => LinPeriodicFault::Off,
                PeriodicArg::Gnd => LinPeriodicFault::ToGnd,
                PeriodicArg::Vbatt => LinPeriodicFault::ToVbatt,
            };
            vh.set_periodic_lin_fault(mode, hz).map_err(e)?;
        }
        Cmd::Fault { sub: None, set } => {
            if set.interrupt.is_none() && set.short_h.is_none() && set.short_l.is_none() {
                return Err(
                    "give --interrupt, --short-h and/or --short-l, or use `fault clear`".into(),
                );
            }
            let interrupt = match set.interrupt.unwrap_or(InterruptArg::None) {
                InterruptArg::None => LineInterrupt::None,
                InterruptArg::H => LineInterrupt::CanH,
                InterruptArg::L => LineInterrupt::CanL,
                InterruptArg::Both => LineInterrupt::Both,
            };
            let h = match set.short_h.unwrap_or(ShortHArg::None) {
                ShortHArg::None => LineShort::None,
                ShortHArg::Gnd => LineShort::Gnd,
                ShortHArg::Vbatt => LineShort::Vbatt,
                ShortHArg::Canl => LineShort::CanL,
            };
            let l = match set.short_l.unwrap_or(ShortLArg::None) {
                ShortLArg::None => LineShort::None,
                ShortLArg::Gnd => LineShort::Gnd,
                ShortLArg::Vbatt => LineShort::Vbatt,
            };
            vh.set_bus_faults(interrupt, h, l).map_err(e)?;
        }
        Cmd::Lin { sub } => match sub {
            LinCmd::GroundOffset { volts } => vh.set_ground_offset(volts).map_err(e)?,
            LinCmd::TesterOffset { volts } => vh.set_tester_ground_offset(volts).map_err(e)?,
            LinCmd::RecessiveLimit { volts } => vh.set_lin_recessive_limit(volts).map_err(e)?,
            LinCmd::Cab { mode } => vh
                .set_lin_cab_supply(match mode {
                    CabArg::Internal => LinCabSupply::Internal,
                    CabArg::Auto => LinCabSupply::ExternalOrInternal,
                    CabArg::External => LinCabSupply::External,
                    CabArg::Mode3 => LinCabSupply::Mode3,
                })
                .map_err(e)?,
        },
        Cmd::Can { sub } => match sub {
            CanCmd::Termination => {
                eprintln!("note: the box goes to its safe state; the ECU loses power");
                let (pos, neg) = vh.measure_can_termination().map_err(e)?;
                println!("{pos:.1} ohm (+)   {neg:.1} ohm (-)");
            }
            CanCmd::Diff { period } => {
                let (rec, dom) = vh.measure_can_diff_voltage(period).map_err(e)?;
                println!("recessive  {rec:.3} V");
                println!("dominant   {dom:.3} V");
            }
            CanCmd::Caps { state } => vh.set_can_capacitors(state == OnOff::On).map_err(e)?,
            CanCmd::Cross { state } => vh.set_can_lines_crossed(state == OnOff::On).map_err(e)?,
        },
        Cmd::Io { sub } => match sub {
            IoCmd::Aout { volts } => vh.set_analog_output(volts).map_err(e)?,
            IoCmd::Dout { level } => vh
                .set_digital_output(matches!(level, Level::High))
                .map_err(e)?,
            IoCmd::Range { range } => vh
                .set_current_range(matches!(range, RangeArg::Auto))
                .map_err(e)?,
        },
        Cmd::Raw { command, reply_len } => {
            let reply = vh.raw(&command, reply_len).map_err(e)?;
            if reply_len > 0 {
                println!("{}", escape(&reply));
            }
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        }
    }
}
