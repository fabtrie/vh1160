//! The driver against the simulator: the exact bytes sent and how replies are
//! read. The protocol comes from Vector's DLL, not from a capture, so these
//! pin it — a regression would drive a box that switches ECU power wrongly.

use std::time::Duration;

use vh1160_core::protocol::Feature;
use vh1160_core::{
    Error, LinCabSupply, LinPeriodicFault, LineInterrupt, LineShort, SimHandle, SimTransport,
    Terminal, Transport, Vh1160,
};

fn open(version: &[u8; 4]) -> (Vh1160, SimHandle) {
    let (t, sim) = SimTransport::for_tests(*version);
    let vh = Vh1160::with_transport(Box::new(t)).unwrap();
    assert_eq!(sim.take_log(), ["t"]);
    (vh, sim)
}

#[test]
fn handshake_reads_firmware_and_revision() {
    let (vh, _) = open(b"2308");
    assert_eq!(vh.version().firmware, (2, 3));
    assert_eq!(vh.version().hw_revision, 8);
    assert!(vh.has(Feature::CrossCanLines));
    let (vh, _) = open(b"1104");
    assert_eq!(vh.version().hw_revision, 4);
    assert!(vh.has(Feature::CanMeasurement) && !vh.has(Feature::CurrentRange));
}

#[test]
fn relays_and_vbatt() {
    let (mut vh, sim) = open(b"2308");
    for t in Terminal::ALL {
        vh.set_relay(t, true, Duration::ZERO).unwrap();
        vh.set_relay(t, false, Duration::ZERO).unwrap();
    }
    vh.set_vbatt(13.5, None).unwrap();
    vh.set_vbatt(4.0, None).unwrap();
    assert_eq!(
        sim.take_log(),
        ["S", "R", "U", "T", "W", "V", "v1350", "v0400"]
    );
    assert!(matches!(
        vh.set_vbatt(28.5, None),
        Err(Error::OutOfRange { .. })
    ));
    assert!(sim.take_log().is_empty());
}

#[test]
fn measurement_follows_relays_and_ecu_sleep() {
    let (mut vh, sim) = open(b"2308");
    sim.set_sleep_after(Duration::from_secs(20));
    let m = vh.measure().unwrap();
    assert_eq!((m.kl30, m.kl15, m.current_ma), (0.0, 0.0, 0.0));

    vh.power_on(Some(12.0)).unwrap();
    let m = vh.measure().unwrap();
    assert_eq!((m.kl30, m.kl15, m.current_ma), (12.0, 12.0, 240.0));
    assert!((m.watts() - 2.88).abs() < 1e-9);

    vh.set_relay(Terminal::Kl15, false, Duration::ZERO).unwrap();
    sim.advance(Duration::from_secs(19));
    assert_eq!(
        vh.measure().unwrap().current_ma,
        240.0,
        "still in follow-up"
    );
    sim.advance(Duration::from_secs(2));
    let m = vh.measure().unwrap();
    assert_eq!(
        (m.kl15, m.current_ma),
        (0.0, 0.4),
        "asleep: 0.35 mA, sent in 0.1 mA steps"
    );
}

#[test]
fn inputs_loop_back_from_outputs_in_the_simulator() {
    let (mut vh, sim) = open(b"2308");
    vh.set_analog_output(7.25).unwrap();
    vh.set_digital_output(true).unwrap();
    let m = vh.measure().unwrap();
    assert_eq!((m.analog_input, m.digital_input), (7.25, true));
    vh.set_digital_output(false).unwrap();
    assert_eq!(sim.take_log(), ["A0725", "E", "w", "D"]);
}

#[test]
fn state_and_overload() {
    let (mut vh, sim) = open(b"2308");
    vh.power_on(None).unwrap();
    assert!(!vh.state().unwrap().overload);
    sim.trigger_overload();
    assert!(vh.state().unwrap().overload);
    assert_eq!(vh.measure().unwrap().kl30, 0.0);
    vh.reset_overload().unwrap();
    assert!(!vh.state().unwrap().overload);
}

#[test]
fn bus_faults() {
    let (mut vh, sim) = open(b"2308");
    vh.set_bus_faults(LineInterrupt::Both, LineShort::None, LineShort::None)
        .unwrap();
    vh.set_bus_faults(LineInterrupt::None, LineShort::Gnd, LineShort::Vbatt)
        .unwrap();
    vh.set_bus_faults(LineInterrupt::CanH, LineShort::CanL, LineShort::Gnd)
        .unwrap();
    vh.clear_bus_faults().unwrap();
    assert_eq!(sim.take_log(), ["c300", "c012", "c180", "c000"]);
}

#[test]
fn offsets_send_value_then_enable_and_a_letter_to_turn_off() {
    let (mut vh, sim) = open(b"2308");
    vh.set_ground_offset(2.0).unwrap();
    assert_eq!(vh.measure().unwrap().kl31, 2.0);
    vh.set_ground_offset(0.0).unwrap();
    vh.set_tester_ground_offset(2.5).unwrap();
    vh.set_tester_ground_offset(0.0).unwrap();
    vh.set_lin_recessive_limit(9.5).unwrap();
    vh.set_lin_recessive_limit(0.0).unwrap();
    assert_eq!(
        sim.take_log(),
        ["i0200", "h", "w", "g", "I0250", "H", "G", "n0950", "m", "l"]
    );
}

#[test]
fn lin_cab_and_periodic_fault() {
    let (mut vh, sim) = open(b"2308");
    for m in [
        LinCabSupply::Internal,
        LinCabSupply::ExternalOrInternal,
        LinCabSupply::External,
        LinCabSupply::Mode3,
    ] {
        vh.set_lin_cab_supply(m).unwrap();
    }
    vh.set_periodic_lin_fault(LinPeriodicFault::ToGnd, 1000.0)
        .unwrap();
    vh.set_periodic_lin_fault(LinPeriodicFault::ToVbatt, 2.0)
        .unwrap();
    vh.set_periodic_lin_fault(LinPeriodicFault::Off, 0.0)
        .unwrap();
    assert!(vh
        .set_periodic_lin_fault(LinPeriodicFault::ToGnd, 60_000.0)
        .is_err());
    assert_eq!(
        sim.take_log(),
        ["L", "M", "N", "O", "p1", "r00200", "p2", "r99995", "p0"]
    );
}

#[test]
fn can_termination_sequence() {
    let (mut vh, sim) = open(b"2308");
    vh.power_on(Some(13.5)).unwrap();
    sim.take_log();
    let (pos, neg) = vh.measure_can_termination().unwrap();
    assert_eq!(
        sim.take_log(),
        ["Z", "v0400", "c321", "w", "c312", "w", "Z"]
    );
    // 33.3 mA on the wire (0.1 mA steps) gives 120.12 ohm.
    assert!(
        (pos - 120.0).abs() < 0.5 && (neg - 120.0).abs() < 0.5,
        "{pos} {neg}"
    );
    assert!(
        !sim.snapshot().relays.iter().any(|&r| r),
        "ends in safe state"
    );
}

#[test]
fn can_diff_voltage_and_switches() {
    let (mut vh, sim) = open(b"2308");
    let (rec, dom) = vh
        .measure_can_diff_voltage(Duration::from_millis(10))
        .unwrap();
    assert_eq!((rec, dom), (0.010, 2.0));
    vh.set_can_capacitors(true).unwrap();
    vh.set_can_lines_crossed(true).unwrap();
    vh.set_current_range(false).unwrap();
    vh.safe_state().unwrap();
    assert_eq!(sim.take_log(), ["z", "u1", "x1", "X1", "a2", "Z"]);
}

#[test]
fn features_need_a_minimum_hw_revision() {
    let (mut vh, sim) = open(b"0001");
    assert!(matches!(
        vh.safe_state(),
        Err(Error::HardwareRevision {
            needed: 2,
            actual: 1,
            ..
        })
    ));
    assert!(vh.set_tester_ground_offset(1.0).is_err());
    let (mut vh3, sim3) = open(b"0003");
    assert!(vh3.set_can_capacitors(true).is_err());
    assert!(vh3.measure_can_diff_voltage(Duration::ZERO).is_err());
    let (mut vh7, sim7) = open(b"0007");
    assert!(vh7.set_can_lines_crossed(true).is_err());
    assert!(sim.take_log().is_empty() && sim3.take_log().is_empty() && sim7.take_log().is_empty());
}

#[test]
fn shut_down_falls_back_below_revision_2() {
    let (mut vh, sim) = open(b"2308");
    vh.shut_down().unwrap();
    assert_eq!(sim.take_log(), ["Z"]);
    let (mut vh, sim) = open(b"0001");
    vh.shut_down().unwrap();
    assert_eq!(sim.take_log(), ["R", "T", "V", "v0000"]);
}

/// Answers the version request with garbage first, then properly.
struct Flaky {
    answers: Vec<&'static [u8]>,
    pending: Vec<u8>,
    sent: usize,
}

impl Transport for Flaky {
    fn clear_input(&mut self) -> vh1160_core::Result<()> {
        self.pending.clear();
        Ok(())
    }
    fn write_all(&mut self, _: &[u8]) -> vh1160_core::Result<()> {
        if let Some(a) = self.answers.get(self.sent) {
            self.pending.extend_from_slice(a);
        }
        self.sent += 1;
        Ok(())
    }
    fn read(&mut self, n: usize) -> vh1160_core::Result<Vec<u8>> {
        let n = n.min(self.pending.len());
        Ok(self.pending.drain(..n).collect())
    }
    fn sleep(&mut self, _: Duration) {}
}

#[test]
fn a_bad_first_answer_is_retried_and_two_fail() {
    let t = Flaky {
        answers: vec![b"x0000\r", b"t2305\r"],
        pending: vec![],
        sent: 0,
    };
    assert_eq!(
        Vh1160::with_transport(Box::new(t))
            .unwrap()
            .version()
            .hw_revision,
        5
    );

    let t = Flaky {
        answers: vec![b"", b"t23"],
        pending: vec![],
        sent: 0,
    };
    let err = Vh1160::with_transport(Box::new(t)).err().unwrap();
    assert!(err.to_string().contains("short answer"), "{err}");
}
