//! MNC-006 transport-posture guard: a public bind must declare a
//! TLS-terminating proxy, or the boot refuses (PHI would travel in cleartext).

use kelson_demo::ensure_transport_posture;

#[test]
fn a_loopback_bind_is_allowed_without_a_proxy() {
    assert!(ensure_transport_posture("127.0.0.1:8180", false).is_ok());
    assert!(ensure_transport_posture("[::1]:8180", false).is_ok());
}

#[test]
fn a_non_loopback_bind_without_a_proxy_refuses_to_boot() {
    let err = ensure_transport_posture("0.0.0.0:8180", false).unwrap_err();
    assert!(err.contains("terminating_proxy"), "{err}");
    assert!(err.contains("0.0.0.0:8180"), "{err}");
}

#[test]
fn a_non_loopback_bind_behind_a_terminating_proxy_is_allowed() {
    assert!(ensure_transport_posture("0.0.0.0:8180", true).is_ok());
    assert!(ensure_transport_posture("10.0.0.5:8180", true).is_ok());
}

#[test]
fn an_unparseable_address_is_left_to_the_bind() {
    // Not an ip:port — the guard does not second-guess it; the bind errors.
    assert!(ensure_transport_posture("not-an-address", false).is_ok());
}
