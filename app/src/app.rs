//! The main window: header bar, controls, LCD readouts, tabs, footer.

use std::collections::VecDeque;
use std::time::Duration;

use futures::StreamExt;
use gpui::{
    Anchor, AnyElement, ClickEvent, Context, Div, Entity, FontWeight, IntoElement, Render,
    SharedString, Subscription, Task, Window, anchored, deferred, div, point, prelude::*, px,
};
use gpui_component::input::{InputEvent, InputState};
use vh1160_core::protocol::Feature;
use vh1160_core::remote::Request;
use vh1160_core::{
    DeviceState, LinCabSupply, LinPeriodicFault, LineInterrupt, LineShort, Measurement, PortEntry,
    Terminal, Version,
};

use crate::chart::{self, Axis, Series};
use crate::settings::Settings;
use crate::theme::{self, MONO_FONT, Palette, RADIUS, ThemeChoice, UI_FONT};
use crate::ui::{self, Led, OnClick};
use crate::worker::{Cmd, Event, POLL, Target, Worker};

/// History kept for the charts and the CSV export.
const HISTORY_S: f64 = 300.0;

pub fn palette_for(choice: ThemeChoice, system_dark: bool) -> Palette {
    let dark = match choice {
        ThemeChoice::System => system_dark,
        ThemeChoice::Light => false,
        ThemeChoice::Dark => true,
    };
    if dark {
        Palette::dark()
    } else {
        Palette::light()
    }
}

fn is_dark(a: gpui::WindowAppearance) -> bool {
    matches!(
        a,
        gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark
    )
}

#[derive(Clone)]
enum Conn {
    Disconnected,
    Connecting,
    Connected {
        version: Version,
        label: String,
        sim: bool,
    },
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Graphs,
    Faults,
    Lin,
    Can,
    Io,
}

#[derive(Clone, Copy)]
struct Sample {
    t: f64,
    v30: f64,
    v15: f64,
    ma: f64,
}

struct Inputs {
    vset: Entity<InputState>,
    reset_ms: Entity<InputState>,
    sim_load: Entity<InputState>,
    periodic_hz: Entity<InputState>,
    gnd: Entity<InputState>,
    tester: Entity<InputState>,
    limit: Entity<InputState>,
    diff_ms: Entity<InputState>,
    aout: Entity<InputState>,
}

pub struct VhApp {
    worker: Worker,
    settings: Settings,
    system_dark: bool,
    inputs: Inputs,
    _events: Task<()>,
    _subs: Vec<Subscription>,

    ports: Vec<PortEntry>,
    /// `None` = simulator.
    port: Option<String>,
    port_menu: bool,
    conn: Conn,
    /// Relay states are not readable from the box: this is what was last sent
    /// (or, right after connecting, inferred from the measured voltages).
    relays: [bool; 3],
    infer_relays: bool,
    busy: Option<&'static str>,
    error: Option<String>,

    last: Option<Measurement>,
    state: Option<DeviceState>,
    overload_dismissed: bool,
    hist: VecDeque<Sample>,
    samples: u64,
    paused: bool,
    window_s: f64,
    log_current: bool,
    y_top_v: f64,
    y_top_i: f64,

    tab: Tab,
    int_h: bool,
    int_l: bool,
    short_h: LineShort,
    short_l: LineShort,
    faults_active: bool,
    periodic: LinPeriodicFault,
    periodic_active: bool,
    gnd_on: bool,
    tester_on: bool,
    limit_on: bool,
    cab: LinCabSupply,
    term: Option<(f64, f64)>,
    diff: Option<(f64, f64)>,
    caps: bool,
    cross: bool,
    dout: bool,
    range_auto: bool,
}

fn input(
    value: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut Context<VhApp>,
) -> Entity<InputState> {
    let value: SharedString = value.into();
    cx.new(|cx| {
        let mut s = InputState::new(window, cx);
        s.set_value(value, window, cx);
        s
    })
}

fn num(state: &Entity<InputState>, cx: &gpui::App) -> Option<f64> {
    state.read(cx).value().trim().replace(',', ".").parse().ok()
}

fn fmt_v(v: f64) -> String {
    format!("{v:.2}")
}

impl VhApp {
    pub fn new(settings: Settings, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (worker, mut events) = Worker::spawn();
        let inputs = Inputs {
            vset: input(fmt_v(settings.vbatt), window, cx),
            reset_ms: input(settings.reset_off_ms.to_string(), window, cx),
            sim_load: input("240", window, cx),
            periodic_hz: input("1000", window, cx),
            gnd: input("2.0", window, cx),
            tester: input("2.0", window, cx),
            limit: input("9.5", window, cx),
            diff_ms: input("1000", window, cx),
            aout: input("5.0", window, cx),
        };
        let enter = |this: &mut Self,
                     ev: &InputEvent,
                     f: fn(&mut Self, &mut Context<Self>),
                     cx: &mut Context<Self>| {
            if let InputEvent::PressEnter { .. } = ev {
                f(this, cx);
            }
        };
        let subs = vec![
            cx.subscribe_in(&inputs.vset, window, move |this, _, ev, _, cx| {
                enter(this, ev, Self::apply_vset, cx)
            }),
            cx.subscribe_in(&inputs.sim_load, window, move |this, _, ev, _, cx| {
                enter(this, ev, Self::apply_sim_load, cx)
            }),
            cx.subscribe_in(&inputs.aout, window, move |this, _, ev, _, cx| {
                enter(this, ev, Self::apply_aout, cx)
            }),
            cx.observe_window_appearance(window, |this, window, cx| {
                this.system_dark = is_dark(window.appearance());
                this.apply_theme(cx);
            }),
        ];
        let task = cx.spawn(async move |this, cx| {
            while let Some(ev) = events.next().await {
                if this.update(cx, |this, cx| this.on_event(ev, cx)).is_err() {
                    break;
                }
            }
        });
        let ports = vh1160_core::list_ports().unwrap_or_default();
        let port = settings
            .port
            .clone()
            .filter(|p| ports.iter().any(|e| &e.name == p));
        Self {
            worker,
            system_dark: is_dark(window.appearance()),
            settings,
            inputs,
            _events: task,
            _subs: subs,
            ports,
            port,
            port_menu: false,
            conn: Conn::Disconnected,
            relays: [false; 3],
            infer_relays: false,
            busy: None,
            error: None,
            last: None,
            state: None,
            overload_dismissed: false,
            hist: VecDeque::new(),
            samples: 0,
            paused: false,
            window_s: 60.0,
            log_current: false,
            y_top_v: 5.0,
            y_top_i: 1.0,
            tab: Tab::Graphs,
            int_h: false,
            int_l: false,
            short_h: LineShort::None,
            short_l: LineShort::None,
            faults_active: false,
            periodic: LinPeriodicFault::Off,
            periodic_active: false,
            gnd_on: false,
            tester_on: false,
            limit_on: false,
            cab: LinCabSupply::Internal,
            term: None,
            diff: None,
            caps: false,
            cross: false,
            dout: false,
            range_auto: true,
        }
    }

    // -- state ---------------------------------------------------------------

    fn connected(&self) -> bool {
        matches!(self.conn, Conn::Connected { .. })
    }

    fn sim(&self) -> bool {
        matches!(self.conn, Conn::Connected { sim: true, .. })
    }

    fn has(&self, f: Feature) -> bool {
        matches!(&self.conn, Conn::Connected { version, .. } if version.hw_revision >= f.min_revision())
    }

    fn ecu_on(&self) -> bool {
        self.relays[0] && self.relays[1]
    }

    fn send(&mut self, cmd: Cmd, cx: &mut Context<Self>) {
        if self.connected() {
            self.worker.send(cmd);
        }
        cx.notify();
    }

    fn apply_theme(&mut self, cx: &mut Context<Self>) {
        theme::apply(palette_for(self.settings.theme, self.system_dark), cx);
        cx.notify();
    }

    fn on_event(&mut self, ev: Event, cx: &mut Context<Self>) {
        match ev {
            Event::Connected {
                version,
                label,
                sim,
            } => {
                self.conn = Conn::Connected {
                    version,
                    label,
                    sim,
                };
                self.error = None;
                self.hist.clear();
                self.samples = 0;
                self.infer_relays = true;
                self.term = None;
                self.diff = None;
                self.state = None;
                if sim {
                    self.apply_sim_load(cx);
                }
            }
            Event::Disconnected { error } => {
                self.conn = Conn::Disconnected;
                self.busy = None;
                if error.is_some() {
                    self.error = error;
                }
            }
            Event::Sample { t, m } => {
                if self.infer_relays {
                    self.infer_relays = false;
                    self.relays = [m.kl30 > 1.0, m.kl15 > 1.0, m.kl30 > 1.0 || m.kl15 > 1.0];
                }
                self.last = Some(m);
                self.samples += 1;
                if !self.paused {
                    self.hist.push_back(Sample {
                        t,
                        v30: m.kl30,
                        v15: m.kl15,
                        ma: m.current_ma,
                    });
                    while self.hist.front().is_some_and(|s| s.t < t - HISTORY_S) {
                        self.hist.pop_front();
                    }
                    self.update_axes();
                }
            }
            Event::State(s) => {
                if !s.overload {
                    self.overload_dismissed = false;
                }
                self.state = Some(s);
            }
            Event::Termination(p, n) => {
                self.term = Some((p, n));
                self.busy = None;
                self.after_safe_state();
            }
            Event::Diff(r, d) => {
                self.diff = Some((r, d));
                self.busy = None;
            }
            Event::Error(e) => {
                self.error = Some(e);
                self.busy = None;
            }
            Event::Remote(req) => self.mirror_remote(req),
        }
        cx.notify();
    }

    /// Updates the switches after the CLI changed the box through the app; the
    /// box cannot report them. Mirrors what the matching action here sets.
    fn mirror_remote(&mut self, req: Request) {
        match req {
            Request::Relay(t, on) => {
                let k = Terminal::ALL.iter().position(|&x| x == t).unwrap_or(0);
                self.relays[k] = on;
            }
            Request::PowerOn(_) | Request::ResetEcu(_) => self.relays = [true; 3],
            Request::PowerOff => {
                self.relays[0] = false;
                self.relays[1] = false;
            }
            Request::Safe => self.after_safe_state(),
            Request::BusFaults(interrupt, h, l) => {
                self.int_h = matches!(interrupt, LineInterrupt::CanH | LineInterrupt::Both);
                self.int_l = matches!(interrupt, LineInterrupt::CanL | LineInterrupt::Both);
                self.short_h = h;
                self.short_l = l;
                self.faults_active = interrupt != LineInterrupt::None
                    || h != LineShort::None
                    || l != LineShort::None;
                if self.faults_active {
                    self.periodic_active = false;
                }
            }
            Request::PeriodicLinFault(mode, _) => {
                self.periodic = mode;
                self.periodic_active = mode != LinPeriodicFault::Off;
                if self.periodic_active {
                    self.faults_active = false;
                }
            }
            Request::GroundOffset(v) => self.mirror_level(0, v > 0.0),
            Request::TesterGroundOffset(v) => self.mirror_level(1, v > 0.0),
            Request::LinRecessiveLimit(v) => self.mirror_level(2, v > 0.0),
            Request::LinCabSupply(m) => self.cab = m,
            Request::DigitalOutput(b) => self.dout = b,
            Request::CurrentRange { auto } => self.range_auto = auto,
            Request::CanCapacitors(b) => self.caps = b,
            Request::CanLinesCrossed(b) => self.cross = b,
            _ => {}
        }
    }

    /// Like [`Self::toggle_level`]: the three level features are exclusive.
    fn mirror_level(&mut self, which: usize, on: bool) {
        if on {
            self.gnd_on = false;
            self.tester_on = false;
            self.limit_on = false;
        }
        match which {
            0 => self.gnd_on = on,
            1 => self.tester_on = on,
            _ => self.limit_on = on,
        }
    }

    fn update_axes(&mut self) {
        let Some(last) = self.hist.back() else { return };
        let since = last.t - self.window_s;
        let visible = self.hist.iter().filter(|s| s.t >= since);
        let (vmax, imax) = visible.fold((0f64, 0f64), |(v, i), s| {
            (v.max(s.v30).max(s.v15), i.max(s.ma))
        });
        self.y_top_v = chart::y_top(self.y_top_v, vmax, 5.0);
        self.y_top_i = chart::y_top(self.y_top_i, imax, 1.0);
    }

    // -- actions ---------------------------------------------------------------

    fn toggle_connection(&mut self, cx: &mut Context<Self>) {
        self.port_menu = false;
        match self.conn {
            Conn::Connected { .. } | Conn::Connecting => {
                self.worker.send(Cmd::Disconnect);
                self.conn = Conn::Disconnected;
            }
            Conn::Disconnected => {
                let target = match &self.port {
                    Some(p) => Target::Port(p.clone()),
                    None => Target::Sim,
                };
                self.settings.port = self.port.clone();
                self.settings.save();
                self.conn = Conn::Connecting;
                self.error = None;
                self.worker.send(Cmd::Connect(target));
            }
        }
        cx.notify();
    }

    fn refresh_ports(&mut self, cx: &mut Context<Self>) {
        self.ports = vh1160_core::list_ports().unwrap_or_default();
        if self
            .port
            .as_ref()
            .is_some_and(|p| !self.ports.iter().any(|e| &e.name == p))
        {
            self.port = None;
        }
        cx.notify();
    }

    fn vset(&mut self, cx: &mut Context<Self>) -> Option<f64> {
        match num(&self.inputs.vset, cx) {
            Some(v) if (2.0..=28.0).contains(&v) => Some(v),
            _ => {
                self.error = Some("VBatt setpoint must be 2.00 to 28.00 V".into());
                cx.notify();
                None
            }
        }
    }

    fn apply_vset(&mut self, cx: &mut Context<Self>) {
        if let Some(v) = self.vset(cx) {
            self.settings.vbatt = v;
            self.settings.save();
            self.send(Cmd::Vbatt(v), cx);
        }
    }

    fn toggle_power(&mut self, cx: &mut Context<Self>) {
        if self.ecu_on() {
            self.relays[0] = false;
            self.relays[1] = false;
            self.send(Cmd::PowerOff, cx);
        } else if let Some(v) = self.vset(cx) {
            self.relays = [true; 3];
            self.send(Cmd::PowerOn(v), cx);
        }
    }

    fn reset_ecu(&mut self, cx: &mut Context<Self>) {
        let ms = num(&self.inputs.reset_ms, cx)
            .unwrap_or(1000.0)
            .clamp(10.0, 60_000.0) as u64;
        self.settings.reset_off_ms = ms;
        self.settings.save();
        self.relays = [true; 3];
        self.send(Cmd::ResetEcu(Duration::from_millis(ms)), cx);
    }

    fn toggle_relay(&mut self, k: usize, cx: &mut Context<Self>) {
        self.relays[k] = !self.relays[k];
        let t = Terminal::ALL[k];
        let on = self.relays[k];
        self.send(Cmd::Relay(t, on), cx);
    }

    /// What the safe state resets.
    fn after_safe_state(&mut self) {
        self.relays = [false; 3];
        self.faults_active = false;
        self.periodic_active = false;
        self.gnd_on = false;
        self.tester_on = false;
        self.limit_on = false;
    }

    fn safe_state(&mut self, cx: &mut Context<Self>) {
        self.after_safe_state();
        self.send(Cmd::Safe, cx);
    }

    fn apply_faults(&mut self, cx: &mut Context<Self>) {
        let interrupt = match (self.int_h, self.int_l) {
            (false, false) => LineInterrupt::None,
            (true, false) => LineInterrupt::CanH,
            (false, true) => LineInterrupt::CanL,
            (true, true) => LineInterrupt::Both,
        };
        let short_l = if self.short_h == LineShort::CanL {
            LineShort::None
        } else {
            self.short_l
        };
        self.faults_active = interrupt != LineInterrupt::None
            || self.short_h != LineShort::None
            || short_l != LineShort::None;
        if self.faults_active {
            self.periodic_active = false;
        }
        self.send(Cmd::Faults(interrupt, self.short_h, short_l), cx);
    }

    fn clear_faults(&mut self, cx: &mut Context<Self>) {
        self.int_h = false;
        self.int_l = false;
        self.short_h = LineShort::None;
        self.short_l = LineShort::None;
        self.apply_faults(cx);
    }

    fn apply_periodic(&mut self, cx: &mut Context<Self>) {
        let hz = num(&self.inputs.periodic_hz, cx).unwrap_or(0.0);
        if self.periodic != LinPeriodicFault::Off && !(2.0..=50_000.0).contains(&hz) {
            self.error = Some("Frequency must be 2 Hz to 50 kHz".into());
            cx.notify();
            return;
        }
        self.periodic_active = self.periodic != LinPeriodicFault::Off;
        if self.periodic_active {
            self.faults_active = false;
        }
        self.send(Cmd::Periodic(self.periodic, hz), cx);
    }

    /// The three level features are exclusive: turning one on turns the others off.
    fn toggle_level(&mut self, which: usize, cx: &mut Context<Self>) {
        let now_on = ![self.gnd_on, self.tester_on, self.limit_on][which];
        let (state, max, name) = match which {
            0 => (&self.inputs.gnd, 10.0, "ECU ground offset"),
            1 => (&self.inputs.tester, 20.0, "Tester ground offset"),
            _ => (&self.inputs.limit, 28.0, "LIN recessive limit"),
        };
        let volts = if now_on {
            match num(state, cx) {
                Some(v) if v > 0.0 && v <= max => v,
                _ => {
                    self.error = Some(format!("{name} must be above 0 and at most {max} V"));
                    cx.notify();
                    return;
                }
            }
        } else {
            0.0
        };
        if now_on {
            self.gnd_on = false;
            self.tester_on = false;
            self.limit_on = false;
        }
        match which {
            0 => self.gnd_on = now_on,
            1 => self.tester_on = now_on,
            _ => self.limit_on = now_on,
        }
        let cmd = match which {
            0 => Cmd::GroundOffset(volts),
            1 => Cmd::TesterOffset(volts),
            _ => Cmd::LinLimit(volts),
        };
        self.send(cmd, cx);
    }

    fn measure_termination(&mut self, cx: &mut Context<Self>) {
        self.busy = Some("termination");
        self.term = None;
        self.send(Cmd::Termination, cx);
    }

    fn measure_diff(&mut self, cx: &mut Context<Self>) {
        let ms = num(&self.inputs.diff_ms, cx)
            .unwrap_or(1000.0)
            .clamp(10.0, 10_000.0) as u64;
        self.busy = Some("diff");
        self.diff = None;
        self.send(Cmd::Diff(Duration::from_millis(ms)), cx);
    }

    fn apply_aout(&mut self, cx: &mut Context<Self>) {
        match num(&self.inputs.aout, cx) {
            Some(v) if (0.0..=15.0).contains(&v) => self.send(Cmd::AnalogOut(v), cx),
            _ => {
                self.error = Some("Analog output must be 0 to 15 V".into());
                cx.notify();
            }
        }
    }

    fn apply_sim_load(&mut self, cx: &mut Context<Self>) {
        if let Some(ma) = num(&self.inputs.sim_load, cx) {
            self.send(Cmd::SimLoad(ma.clamp(0.0, 5000.0)), cx);
        }
    }

    fn export_csv(&mut self, cx: &mut Context<Self>) {
        let mut csv = String::from("time_s,kl30_v,kl15_v,current_ma\n");
        for s in &self.hist {
            csv.push_str(&format!(
                "{:.3},{:.2},{:.2},{:.1}\n",
                s.t, s.v30, s.v15, s.ma
            ));
        }
        let dir = std::env::var_os("USERPROFILE")
            .map(|h| std::path::PathBuf::from(h).join("Documents"))
            .filter(|d| d.is_dir())
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let rx = cx.prompt_for_new_path(&dir, Some("vh1160.csv"));
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(path))) = rx.await
                && let Err(e) = std::fs::write(&path, csv)
            {
                let _ = this.update(cx, |this, cx| {
                    this.error = Some(format!("{}: {e}", path.display()));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    // -- render ----------------------------------------------------------------

    fn header(&self, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let theme_items: Vec<(Option<&'static str>, SharedString, OnClick)> = [
            (ThemeChoice::System, Some("device-desktop"), "System"),
            (ThemeChoice::Light, Some("sun"), ""),
            (ThemeChoice::Dark, Some("moon"), ""),
        ]
        .into_iter()
        .map(|(choice, icon, text)| {
            let f: OnClick = Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.settings.theme = choice;
                this.settings.save();
                this.apply_theme(cx);
            }));
            (icon, SharedString::from(text), f)
        })
        .collect();
        let selected = match self.settings.theme {
            ThemeChoice::System => 0,
            ThemeChoice::Light => 1,
            ThemeChoice::Dark => 2,
        };
        let disconnected = matches!(self.conn, Conn::Disconnected);
        let (pill_text, pill_bg, pill_fg): (SharedString, _, _) = match &self.conn {
            Conn::Disconnected => ("Disconnected".into(), p.hover, p.tx2),
            Conn::Connecting => ("Connecting…".into(), p.hover, p.tx2),
            Conn::Connected {
                sim: true, version, ..
            } => (
                format!(
                    "Simulator · FW {}.{} · HW rev {}",
                    version.firmware.0, version.firmware.1, version.hw_revision
                )
                .into(),
                p.ok_bg,
                p.ok_tx,
            ),
            Conn::Connected { version, .. } => (
                format!(
                    "Connected · FW {}.{} · HW rev {}",
                    version.firmware.0, version.firmware.1, version.hw_revision
                )
                .into(),
                p.ok_bg,
                p.ok_tx,
            ),
        };
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.0))
            .px(px(16.0))
            .py(px(10.0))
            .border_b_1()
            .border_color(p.bd)
            .bg(p.bg1)
            .child(ui::icon("circuit-switch-open", 20.0, p.tx))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("VH1160 control"),
            )
            .child(div().flex_1())
            .child(ui::segmented("theme", theme_items, selected, true, p))
            .child(self.port_select(disconnected, p, cx))
            .child(ui::action(
                "refresh",
                Some("refresh"),
                "",
                disconnected,
                p,
                cx.listener(|this, _: &ClickEvent, _, cx| this.refresh_ports(cx)),
            ))
            .child(ui::action(
                "connect",
                Some(if disconnected {
                    "plug-connected"
                } else {
                    "plug"
                }),
                if disconnected {
                    "Connect"
                } else {
                    "Disconnect"
                },
                true,
                p,
                cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_connection(cx)),
            ))
            .child(
                div()
                    .text_size(px(12.0))
                    .px(px(10.0))
                    .py(px(3.0))
                    .rounded(px(RADIUS))
                    .bg(pill_bg)
                    .text_color(pill_fg)
                    .child(pill_text),
            )
    }

    fn port_label(&self, port: &Option<String>) -> SharedString {
        match port {
            None => "Simulator".into(),
            Some(name) => match self.ports.iter().find(|e| &e.name == name) {
                Some(e) if e.ftdi => format!("{name} · FTDI · {}", e.description).into(),
                Some(e) => format!("{name} · {}", e.description).into(),
                None => name.clone().into(),
            },
        }
    }

    fn port_select(&self, enabled: bool, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let trigger = ui::button("port", p)
            .w(px(230.0))
            .justify_between()
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(self.port_label(&self.port)),
            )
            .child(ui::icon("chevron-down", 14.0, p.tx2));
        let trigger = ui::enabled_or_dimmed(
            trigger,
            enabled,
            cx.listener(|this, _: &ClickEvent, _, cx| {
                this.port_menu = !this.port_menu;
                cx.notify();
            }),
        );
        let mut options: Vec<Option<String>> =
            self.ports.iter().map(|e| Some(e.name.clone())).collect();
        options.push(None);
        let hover = p.hover;
        let menu = div()
            .id("port-menu")
            .flex()
            .flex_col()
            .w(px(300.0))
            .py(px(4.0))
            .bg(p.bg)
            .border_1()
            .border_color(p.bd)
            .rounded(px(RADIUS))
            .shadow_md()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.port_menu = false;
                cx.notify();
            }))
            .children(options.into_iter().enumerate().map(|(k, opt)| {
                let current = opt == self.port;
                let label = self.port_label(&opt);
                div()
                    .id(("port-opt", k))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(10.0))
                    .py(px(6.0))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .child(
                        div()
                            .w(px(14.0))
                            .when(current, |d| d.child(ui::icon("check", 14.0, p.tx))),
                    )
                    .child(label)
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.port = opt.clone();
                        this.port_menu = false;
                        cx.notify();
                    }))
            }));
        div()
            .relative()
            .flex_none()
            .child(trigger)
            .when(self.port_menu && enabled, |d| {
                d.child(
                    deferred(
                        anchored()
                            .anchor(Anchor::TopLeft)
                            .offset(point(px(0.0), px(2.0)))
                            .snap_to_window_with_margin(px(4.0))
                            .child(menu),
                    )
                    .with_priority(2),
                )
            })
    }

    fn banners(&self, p: &Palette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        if self.state.is_some_and(|s| s.overload) && !self.overload_dismissed && self.connected() {
            out.push(
                ui::banner(p.warn_bg, p.warn_tx)
                    .child(ui::icon("alert-triangle", 16.0, p.warn_tx))
                    .child(div().flex_1().child(
                        "Overload: the box switched its outputs off. Remove the cause, then reset.",
                    ))
                    .child(ui::action(
                        "reset-overload",
                        None,
                        "Reset overload",
                        true,
                        p,
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.send(Cmd::ResetOverload, cx)
                        }),
                    ))
                    .child(
                        div()
                            .id("dismiss-overload")
                            .cursor_pointer()
                            .child(ui::icon("x", 16.0, p.warn_tx))
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.overload_dismissed = true;
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
            );
        }
        if let Some(e) = &self.error {
            out.push(
                ui::banner(p.err_bg, p.err_tx)
                    .child(div().flex_1().child(e.clone()))
                    .child(
                        div()
                            .id("dismiss-error")
                            .cursor_pointer()
                            .child(ui::icon("x", 16.0, p.err_tx))
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.error = None;
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
            );
        }
        out
    }

    fn controls(&self, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let on = self.connected();
        let relay_row = |k: usize, what: &'static str, cx: &mut Context<Self>| {
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .px(px(10.0))
                .py(px(7.0))
                .when(k > 0, |d| d.border_t_1().border_color(p.bd))
                .child(
                    div()
                        .w(px(40.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(Terminal::ALL[k].label()),
                )
                .child(
                    div()
                        .flex_1()
                        .text_size(px(13.0))
                        .text_color(p.tx2)
                        .child(what),
                )
                .child(ui::switch(
                    ("relay", k),
                    self.relays[k],
                    on,
                    p,
                    Box::new(
                        cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_relay(k, cx)),
                    ),
                ))
        };
        div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .w(px(240.0))
            .flex_none()
            .child(ui::section("Supply", p))
            .child(
                div()
                    .child(ui::caption("VBatt setpoint (2.00 to 28.00 V)", p))
                    .child(ui::field(&self.inputs.vset, "V", "VBatt setpoint", p)),
            )
            .child(ui::action(
                "apply-vset",
                None,
                "Apply setpoint",
                on,
                p,
                cx.listener(|this, _: &ClickEvent, _, cx| this.apply_vset(cx)),
            ))
            .child(ui::power_button(
                "power",
                self.ecu_on(),
                on,
                p,
                Box::new(cx.listener(|this, _: &ClickEvent, _, cx| this.toggle_power(cx))),
            ))
            .child(ui::action(
                "reset-ecu",
                Some("rotate-clockwise"),
                "Reset ECU",
                on,
                p,
                cx.listener(|this, _: &ClickEvent, _, cx| this.reset_ecu(cx)),
            ))
            .child(
                div()
                    .child(ui::caption("Reset off time", p))
                    .child(ui::field(&self.inputs.reset_ms, "ms", "Reset off time", p)),
            )
            .child(ui::section("Terminals", p))
            .child(
                div()
                    .border_1()
                    .border_color(p.bd)
                    .rounded(px(RADIUS))
                    .child(relay_row(0, "VBatt", cx))
                    .child(relay_row(1, "Ignition", cx))
                    .child(relay_row(2, "Ground", cx)),
            )
            .child(ui::action(
                "safe",
                Some("shield-check"),
                "Safe state",
                on && self.has(Feature::SafeState),
                p,
                cx.listener(|this, _: &ClickEvent, _, cx| this.safe_state(cx)),
            ))
            .when(self.sim(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.0))
                        .pt(px(10.0))
                        .border_t_1()
                        .border_color(p.bd)
                        .child(ui::caption("ECU load (simulation only)", p))
                        .child(ui::field(
                            &self.inputs.sim_load,
                            "mA",
                            "Simulated ECU load",
                            p,
                        ))
                        .child(ui::muted("Sleeps 0.35 mA, 20 s after KL15 opens.", p))
                        .child(
                            div()
                                .flex()
                                .gap(px(8.0))
                                .child(ui::action(
                                    "sim-load",
                                    None,
                                    "Apply",
                                    true,
                                    p,
                                    cx.listener(|this, _: &ClickEvent, _, cx| {
                                        this.apply_sim_load(cx)
                                    }),
                                ))
                                .child(ui::action(
                                    "sim-overload",
                                    Some("alert-triangle"),
                                    "Overload",
                                    true,
                                    p,
                                    cx.listener(|this, _: &ClickEvent, _, cx| {
                                        this.send(Cmd::SimOverload, cx)
                                    }),
                                )),
                        ),
                )
            })
    }

    fn readouts(&self, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let m = self.last.filter(|_| self.connected());
        let v = |f: fn(&Measurement) -> f64| {
            m.map(|m| format!("{:.2}", f(&m)))
                .unwrap_or_else(|| "0.00".into())
        };
        let ma = m.map(|m| m.current_ma).unwrap_or(0.0);
        let current = if ma >= 1000.0 {
            format!("{ma:.0}")
        } else {
            format!("{ma:.1}")
        };
        let watts = m.map(|m| m.watts()).unwrap_or(0.0);
        let fault = self.faults_active || self.periodic_active;
        let overload = self.state.is_some_and(|s| s.overload) && self.connected();
        let relays = if self.connected() {
            self.relays
        } else {
            [false; 3]
        };

        let tabs = [
            (Tab::Graphs, "tab-graphs", "chart-line", "Graphs", None),
            (
                Tab::Faults,
                "tab-faults",
                "bolt-off",
                "Bus faults",
                fault.then_some("active"),
            ),
            (Tab::Lin, "tab-lin", "route", "LIN", None),
            (Tab::Can, "tab-can", "ruler-measure", "CAN", None),
            (Tab::Io, "tab-io", "cpu", "I/O", None),
        ];
        let tab_row = div()
            .flex()
            .gap(px(2.0))
            .border_b_1()
            .border_color(p.bd)
            .mb(px(10.0))
            .children(tabs.map(|(tab, id, icon, text, badge)| {
                ui::tab(
                    id,
                    icon,
                    text,
                    self.tab == tab,
                    badge,
                    p,
                    Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.tab = tab;
                        cx.notify();
                    })),
                )
            }));

        let panel: AnyElement = match self.tab {
            Tab::Graphs => self.graphs(p, cx).into_any_element(),
            Tab::Faults => self.faults_panel(p, cx).into_any_element(),
            Tab::Lin => self.lin_panel(p, cx).into_any_element(),
            Tab::Can => self.can_panel(p, cx).into_any_element(),
            Tab::Io => self.io_panel(p, cx).into_any_element(),
        };

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .child(
                div()
                    .flex()
                    .gap(px(12.0))
                    .mb(px(12.0))
                    .child(ui::lcd("KL30", v(|m| m.kl30), "V", p))
                    .child(ui::lcd("KL15", v(|m| m.kl15), "V", p))
                    .child(ui::lcd("Current", current, "mA", p)),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.0))
                    .mb(px(14.0))
                    .child(ui::indicator("KL30", relays[0], Led::On, p))
                    .child(ui::indicator("KL15", relays[1], Led::On, p))
                    .child(ui::indicator("KL31", relays[2], Led::On, p))
                    .child(ui::indicator(
                        "Bus fault",
                        fault && self.connected(),
                        Led::Fault,
                        p,
                    ))
                    .child(ui::indicator("Overload", overload, Led::Fault, p))
                    .child(div().flex_1())
                    .child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap(px(6.0))
                            .text_size(px(13.0))
                            .text_color(p.tx2)
                            .child("Power")
                            .child(
                                div()
                                    .font_family(MONO_FONT)
                                    .text_size(px(17.0))
                                    .text_color(p.tx)
                                    .child(format!("{watts:.2} W")),
                            ),
                    ),
            )
            .child(tab_row)
            .child(panel)
    }

    fn legend(color: gpui::Hsla, text: &'static str, p: &Palette) -> Div {
        div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .text_size(px(12.0))
            .text_color(p.tx2)
            .child(div().size(px(10.0)).rounded(px(2.0)).bg(color))
            .child(text)
    }

    fn graphs(&self, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let t_end = self.hist.back().map(|s| s.t).unwrap_or(self.window_s);
        let pts = |f: fn(&Sample) -> f64| self.hist.iter().map(|s| (s.t, f(s))).collect::<Vec<_>>();
        let window_items: Vec<(Option<&'static str>, SharedString, OnClick)> =
            [(30.0, "30 s"), (60.0, "60 s"), (300.0, "5 min")]
                .into_iter()
                .map(|(secs, text)| {
                    let f: OnClick = Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.window_s = secs;
                        this.update_axes();
                        cx.notify();
                    }));
                    (None, SharedString::from(text), f)
                })
                .collect();
        let window_sel = match self.window_s as u32 {
            30 => 0,
            60 => 1,
            _ => 2,
        };
        let scale_items: Vec<(Option<&'static str>, SharedString, OnClick)> =
            [(false, "Linear"), (true, "Log")]
                .into_iter()
                .map(|(log, text)| {
                    let f: OnClick = Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.log_current = log;
                        cx.notify();
                    }));
                    (None, SharedString::from(text), f)
                })
                .collect();
        let head = || {
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(10.0))
                .my(px(6.0))
        };
        div()
            .flex()
            .flex_col()
            .child(
                head()
                    .child(Self::legend(p.series_v, "KL30 (V)", p))
                    .child(Self::legend(p.series_v2, "KL15 (V)", p))
                    .child(div().flex_1())
                    .child(ui::segmented("window", window_items, window_sel, true, p))
                    .child(ui::action(
                        "pause",
                        Some(if self.paused {
                            "player-play"
                        } else {
                            "player-pause"
                        }),
                        if self.paused { "Resume" } else { "Pause" },
                        true,
                        p,
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.paused = !this.paused;
                            cx.notify();
                        }),
                    ))
                    .child(ui::action(
                        "csv",
                        Some("download"),
                        "CSV",
                        !self.hist.is_empty(),
                        p,
                        cx.listener(|this, _: &ClickEvent, _, cx| this.export_csv(cx)),
                    )),
            )
            .child(chart::chart(
                vec![
                    Series {
                        color: p.series_v,
                        points: pts(|s| s.v30),
                    },
                    Series {
                        color: p.series_v2,
                        points: pts(|s| s.v15),
                    },
                ],
                Axis {
                    t_end,
                    window_s: self.window_s,
                    y_top: self.y_top_v,
                    log: false,
                },
                p,
            ))
            .child(
                head()
                    .child(Self::legend(p.series_i, "Current (mA)", p))
                    .child(div().flex_1())
                    .child(ui::segmented(
                        "scale",
                        scale_items,
                        usize::from(self.log_current),
                        true,
                        p,
                    )),
            )
            .child(chart::chart(
                vec![Series {
                    color: p.series_i,
                    points: pts(|s| s.ma),
                }],
                Axis {
                    t_end,
                    window_s: self.window_s,
                    y_top: if self.log_current {
                        10f64.powf(self.y_top_i.log10().ceil()).max(1.0)
                    } else {
                        self.y_top_i
                    },
                    log: self.log_current,
                },
                p,
            ))
    }

    fn rev_note(f: Feature) -> Option<String> {
        Some(format!("HW rev ≥ {}", f.min_revision()))
    }

    fn cards() -> Div {
        div().flex().flex_wrap().gap(px(12.0))
    }

    #[allow(clippy::too_many_arguments)]
    fn radio_row<T: Copy + PartialEq + 'static>(
        &self,
        id: &'static str,
        options: &[(T, &'static str)],
        current: T,
        enabled: bool,
        set: fn(&mut Self, T),
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .flex()
            .flex_wrap()
            .gap_x(px(14.0))
            .gap_y(px(4.0))
            .children(options.iter().enumerate().map(|(k, &(v, text))| {
                ui::check(
                    (id, k),
                    v == current,
                    true,
                    text,
                    enabled,
                    p,
                    Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        set(this, v);
                        cx.notify();
                    })),
                )
            }))
    }

    fn faults_panel(&self, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let on = self.connected();
        let periodic_ok = on && self.has(Feature::PeriodicLinFault);
        let periodic_items: Vec<(Option<&'static str>, SharedString, OnClick)> = [
            (LinPeriodicFault::Off, "Off"),
            (LinPeriodicFault::ToGnd, "to GND"),
            (LinPeriodicFault::ToVbatt, "to VBatt"),
        ]
        .into_iter()
        .map(|(m, text)| {
            let f: OnClick = Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.periodic = m;
                cx.notify();
            }));
            (None, SharedString::from(text), f)
        })
        .collect();
        let periodic_sel = match self.periodic {
            LinPeriodicFault::Off => 0,
            LinPeriodicFault::ToGnd => 1,
            LinPeriodicFault::ToVbatt => 2,
        };
        Self::cards()
            .child(
                ui::card("bolt-off", "Line faults", None, p)
                    .child(
                        div().child(ui::caption("Interrupt", p)).child(
                            div()
                                .flex()
                                .gap(px(14.0))
                                .child(ui::check(
                                    "int-h",
                                    self.int_h,
                                    false,
                                    "CAN_H / LIN",
                                    on,
                                    p,
                                    Box::new(cx.listener(|this, _: &ClickEvent, _, cx| {
                                        this.int_h = !this.int_h;
                                        cx.notify();
                                    })),
                                ))
                                .child(ui::check(
                                    "int-l",
                                    self.int_l,
                                    false,
                                    "CAN_L",
                                    on,
                                    p,
                                    Box::new(cx.listener(|this, _: &ClickEvent, _, cx| {
                                        this.int_l = !this.int_l;
                                        cx.notify();
                                    })),
                                )),
                        ),
                    )
                    .child(div().child(ui::caption("Short CAN_H / LIN to", p)).child(
                        self.radio_row(
                            "short-h",
                            &[
                                (LineShort::None, "nothing"),
                                (LineShort::Gnd, "GND"),
                                (LineShort::Vbatt, "VBatt"),
                                (LineShort::CanL, "CAN_L"),
                            ],
                            self.short_h,
                            on,
                            |this, v| this.short_h = v,
                            p,
                            cx,
                        ),
                    ))
                    .child(
                        div()
                            .child(ui::caption("Short CAN_L to", p))
                            .child(self.radio_row(
                                "short-l",
                                &[
                                    (LineShort::None, "nothing"),
                                    (LineShort::Gnd, "GND"),
                                    (LineShort::Vbatt, "VBatt"),
                                ],
                                self.short_l,
                                on && self.short_h != LineShort::CanL,
                                |this, v| this.short_l = v,
                                p,
                                cx,
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(8.0))
                            .child(
                                ui::action(
                                    "faults-apply",
                                    None,
                                    "Apply",
                                    on,
                                    p,
                                    cx.listener(|this, _: &ClickEvent, _, cx| {
                                        this.apply_faults(cx)
                                    }),
                                )
                                .flex_1(),
                            )
                            .child(
                                ui::action(
                                    "faults-clear",
                                    None,
                                    "Clear all",
                                    on,
                                    p,
                                    cx.listener(|this, _: &ClickEvent, _, cx| {
                                        this.clear_faults(cx)
                                    }),
                                )
                                .flex_1(),
                            ),
                    ),
            )
            .child(
                ui::card(
                    "wave-square",
                    "Periodic LIN short",
                    Self::rev_note(Feature::PeriodicLinFault),
                    p,
                )
                .child(div().flex().child(ui::segmented(
                    "periodic",
                    periodic_items,
                    periodic_sel,
                    periodic_ok,
                    p,
                )))
                .child(
                    div()
                        .child(ui::caption("Frequency (2 Hz to 50 kHz)", p))
                        .child(ui::field(
                            &self.inputs.periodic_hz,
                            "Hz",
                            "Periodic short frequency",
                            p,
                        )),
                )
                .child(ui::muted(
                    "Each short lasts 1/(2f): 0.5 ms at 1 kHz. Replaces the line faults.",
                    p,
                ))
                .child(ui::action(
                    "periodic-apply",
                    None,
                    "Apply",
                    periodic_ok,
                    p,
                    cx.listener(|this, _: &ClickEvent, _, cx| this.apply_periodic(cx)),
                )),
            )
    }

    fn lin_panel(&self, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let on = self.connected();
        let level = |k: usize,
                     label: &'static str,
                     state: &Entity<InputState>,
                     active: bool,
                     enabled: bool,
                     cx: &mut Context<Self>| {
            ui::row(label, p)
                .child(div().w(px(130.0)).child(ui::field(state, "V", label, p)))
                .child(ui::switch(
                    ("level", k),
                    active,
                    enabled,
                    p,
                    Box::new(
                        cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_level(k, cx)),
                    ),
                ))
        };
        let kl31 = self
            .last
            .filter(|_| on)
            .map(|m| format!("{:.2} V", m.kl31))
            .unwrap_or_else(|| "—".into());
        Self::cards()
            .child(
                ui::card("route", "Levels", None, p)
                    .child(level(0, "ECU ground offset (0 to 10 V)", &self.inputs.gnd, self.gnd_on, on, cx))
                    .child(level(
                        1,
                        "Tester ground offset (0 to 20 V)",
                        &self.inputs.tester,
                        self.tester_on,
                        on && self.has(Feature::TesterGroundOffset),
                        cx,
                    ))
                    .child(level(2, "LIN recessive limit (0 to 28 V)", &self.inputs.limit, self.limit_on, on, cx))
                    .child(ui::muted("Only one of these is active at a time: turning one on turns the others off.", p)),
            )
            .child(
                ui::card("plug-connected", "LINcab supply", None, p)
                    .child(
                        div().flex().flex_col().gap(px(6.0)).children(
                            [
                                (LinCabSupply::Internal, "Internal 12 V"),
                                (LinCabSupply::ExternalOrInternal, "External if > 12 V on pin 9, else internal"),
                                (LinCabSupply::External, "External only (pin 9)"),
                            ]
                            .into_iter()
                            .enumerate()
                            .map(|(k, (mode, text))| {
                                ui::check(
                                    ("cab", k),
                                    self.cab == mode,
                                    true,
                                    text,
                                    on,
                                    p,
                                    Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        this.cab = mode;
                                        this.send(Cmd::Cab(mode), cx);
                                    })),
                                )
                            }),
                        ),
                    )
                    .child(ui::row("KL31' (ECU ground) voltage", p).child(ui::result_text(kl31))),
            )
    }

    fn can_panel(&self, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let on = self.connected();
        let can_ok = on && self.has(Feature::CanMeasurement) && self.busy.is_none();
        let ohm = |v: Option<f64>| v.map(|v| format!("{v:.1} Ω")).unwrap_or_else(|| "—".into());
        let volt = |v: Option<f64>| v.map(|v| format!("{v:.3} V")).unwrap_or_else(|| "—".into());
        Self::cards()
            .child(
                ui::card(
                    "ruler-measure",
                    "Termination",
                    Self::rev_note(Feature::CanMeasurement),
                    p,
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .px(px(10.0))
                        .py(px(6.0))
                        .rounded(px(RADIUS))
                        .bg(p.warn_bg)
                        .text_color(p.warn_tx)
                        .text_size(px(12.0))
                        .child(ui::icon("alert-triangle", 14.0, p.warn_tx))
                        .child(div().flex_1().min_w(px(0.0)).child(
                            "Measuring switches the box to its safe state: the ECU loses power.",
                        )),
                )
                .child(
                    ui::row("Positive polarity", p)
                        .child(ui::result_text(ohm(self.term.map(|t| t.0)))),
                )
                .child(
                    ui::row("Negative polarity", p)
                        .child(ui::result_text(ohm(self.term.map(|t| t.1)))),
                )
                .child(ui::action(
                    "term",
                    None,
                    if self.busy == Some("termination") {
                        "Measuring…"
                    } else {
                        "Measure termination"
                    },
                    can_ok,
                    p,
                    cx.listener(|this, _: &ClickEvent, _, cx| this.measure_termination(cx)),
                )),
            )
            .child(
                ui::card(
                    "activity",
                    "Differential voltage",
                    Self::rev_note(Feature::CanMeasurement),
                    p,
                )
                .child(
                    div()
                        .child(ui::caption("Measurement period", p))
                        .child(ui::field(
                            &self.inputs.diff_ms,
                            "ms",
                            "Measurement period",
                            p,
                        )),
                )
                .child(ui::row("Recessive", p).child(ui::result_text(volt(self.diff.map(|d| d.0)))))
                .child(ui::row("Dominant", p).child(ui::result_text(volt(self.diff.map(|d| d.1)))))
                .child(ui::action(
                    "diff",
                    None,
                    if self.busy == Some("diff") {
                        "Measuring…"
                    } else {
                        "Measure"
                    },
                    can_ok,
                    p,
                    cx.listener(|this, _: &ClickEvent, _, cx| this.measure_diff(cx)),
                )),
            )
            .child(
                ui::card("switch-horizontal", "Line options", None, p)
                    .child(
                        ui::row("1.6 nF to GND on CAN_H and CAN_L", p).child(ui::switch(
                            "caps",
                            self.caps,
                            on && self.has(Feature::CanCapacitors),
                            p,
                            Box::new(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.caps = !this.caps;
                                let v = this.caps;
                                this.send(Cmd::Capacitors(v), cx);
                            })),
                        )),
                    )
                    .child(
                        ui::row("Cross CAN_H and CAN_L (HW rev ≥ 8)", p).child(ui::switch(
                            "cross",
                            self.cross,
                            on && self.has(Feature::CrossCanLines),
                            p,
                            Box::new(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.cross = !this.cross;
                                let v = this.cross;
                                this.send(Cmd::CrossLines(v), cx);
                            })),
                        )),
                    ),
            )
    }

    fn io_panel(&self, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let on = self.connected();
        let m = self.last.filter(|_| on);
        let range_items: Vec<(Option<&'static str>, SharedString, OnClick)> =
            [(true, "Automatic"), (false, "High range")]
                .into_iter()
                .map(|(auto, text)| {
                    let f: OnClick = Box::new(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.range_auto = auto;
                        this.send(Cmd::CurrentRange { auto }, cx);
                    }));
                    (None, SharedString::from(text), f)
                })
                .collect();
        Self::cards()
            .child(
                ui::card("cpu", "Outputs", None, p)
                    .child(
                        ui::row("Analog output (0 to 15 V)", p)
                            .child(div().w(px(130.0)).child(ui::field(
                                &self.inputs.aout,
                                "V",
                                "Analog output",
                                p,
                            )))
                            .child(ui::action(
                                "aout",
                                None,
                                "Set",
                                on,
                                p,
                                cx.listener(|this, _: &ClickEvent, _, cx| this.apply_aout(cx)),
                            )),
                    )
                    .child(ui::row("Digital output", p).child(ui::switch(
                        "dout",
                        self.dout,
                        on,
                        p,
                        Box::new(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.dout = !this.dout;
                            let v = this.dout;
                            this.send(Cmd::DigitalOut(v), cx);
                        })),
                    ))),
            )
            .child(
                ui::card("activity", "Inputs", None, p)
                    .child(
                        ui::row("Analog input", p).child(ui::result_text(
                            m.map(|m| format!("{:.2} V", m.analog_input))
                                .unwrap_or_else(|| "—".into()),
                        )),
                    )
                    .child(ui::row("Digital input", p).child({
                        let high = m.is_some_and(|m| m.digital_input);
                        ui::indicator(if high { "High" } else { "Low" }, high, Led::Input, p)
                    })),
            )
            .child(
                ui::card(
                    "chart-line",
                    "Current measurement",
                    Self::rev_note(Feature::CurrentRange),
                    p,
                )
                .child(div().flex().child(ui::segmented(
                    "range",
                    range_items,
                    usize::from(!self.range_auto),
                    on && self.has(Feature::CurrentRange),
                    p,
                )))
                .child(ui::muted(
                    "The low range cannot be selected; automatic is the default.",
                    p,
                )),
            )
    }

    fn footer(&self, p: &Palette) -> impl IntoElement {
        let right: SharedString = match &self.conn {
            Conn::Connected {
                label, sim: false, ..
            } => format!("{label} · 230400 baud").into(),
            Conn::Connected { sim: true, .. } => "Simulator".into(),
            Conn::Connecting => "Connecting…".into(),
            Conn::Disconnected => "Not connected".into(),
        };
        div()
            .flex()
            .gap(px(6.0))
            .px(px(16.0))
            .py(px(6.0))
            .border_t_1()
            .border_color(p.bd)
            .bg(p.bg1)
            .text_size(px(12.0))
            .text_color(p.tx3)
            .child(format!(
                "Polling every {} ms · {} samples",
                POLL.as_millis(),
                self.samples
            ))
            .child(div().flex_1())
            .child(right)
    }
}

impl Render for VhApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = cx.global::<Palette>().clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(p.bg)
            .text_color(p.tx)
            .font_family(UI_FONT)
            .text_size(px(14.0))
            .child(self.header(&p, cx))
            .children(self.banners(&p, cx))
            .child(
                div()
                    .id("main")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap(px(20.0))
                            .p(px(16.0))
                            .child(self.controls(&p, cx))
                            .child(self.readouts(&p, cx)),
                    ),
            )
            .child(self.footer(&p))
    }
}
