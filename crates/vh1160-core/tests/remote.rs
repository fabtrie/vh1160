//! A server and a client over a real local socket, with the simulator behind it.

use std::sync::Mutex;
use std::time::Duration;

use vh1160_core::remote::{self, Client, Peer, Reply, Request};
use vh1160_core::{Terminal, Vh1160};

fn start(name: &str) -> Client {
    let (vh, _sim) = Vh1160::simulated();
    let version = vh.version();
    let vh = Mutex::new(vh);
    remote::serve(name, move |req| match req {
        Request::Hello => Reply::Hello(Some(Peer {
            port: None,
            version,
        })),
        req => match req.execute(&mut vh.lock().unwrap()) {
            Ok(r) => r,
            Err(e) => Reply::Error(e.to_string()),
        },
    })
    .unwrap();
    Client::connect_to(name).unwrap()
}

fn unique(tag: &str) -> String {
    format!("vh1160-test-{tag}-{}", std::process::id())
}

#[test]
fn requests_run_on_the_server_device() {
    let mut c = start(&unique("run"));
    let Reply::Hello(Some(peer)) = c.call(&Request::Hello).unwrap() else {
        panic!("no hello");
    };
    assert_eq!(peer.port, None);
    assert_eq!(peer.version.hw_revision, 8);

    assert_eq!(c.call(&Request::Vbatt(13.5)).unwrap(), Reply::Done);
    assert_eq!(c.call(&Request::PowerOn(None)).unwrap(), Reply::Done);
    let Reply::Measurement(m) = c.call(&Request::Measure).unwrap() else {
        panic!("no measurement");
    };
    assert!((m.kl30 - 13.5).abs() < 0.2, "{m:?}");
    assert_eq!(
        c.call(&Request::Relay(Terminal::Kl15, false)).unwrap(),
        Reply::Done
    );
    assert_eq!(
        c.call(&Request::ResetEcu(Duration::from_millis(10)))
            .unwrap(),
        Reply::Done
    );
}

#[test]
fn errors_come_back_and_clients_share_the_server() {
    let name = unique("err");
    let mut c = start(&name);
    let Reply::Error(e) = c.call(&Request::Vbatt(99.0)).unwrap() else {
        panic!("no error");
    };
    assert!(e.contains("out of range"), "{e}");
    // A second client on the same server, while the first stays connected.
    let mut c2 = Client::connect_to(&name).unwrap();
    assert!(matches!(c2.call(&Request::State).unwrap(), Reply::State(_)));
    assert!(matches!(
        c.call(&Request::Info).unwrap(),
        Reply::Info { .. }
    ));
}

#[test]
fn a_second_server_cannot_take_the_name() {
    let name = unique("dup");
    let _c = start(&name);
    assert!(remote::serve(&name, |_| Reply::Done).is_err());
}

#[test]
fn connecting_without_a_server_fails_at_once() {
    assert!(Client::connect_to(&unique("none")).is_err());
}
