// sovereign-systemd: System service adapter for Sovereign.
//
// Implements `sovereign_core::ports::SystemServicePort` for
// Ubuntu/Debian via apt + systemd. Each adapter module wraps a
// single system service: apt package management, systemd unit
// management, and per-service adapters (nginx, mysql, mariadb,
// redis, vsftpd, letsencrypt, phpmyadmin).
//
// Per `docs/phase-0.6.md` §S1, this crate is the Linux-only
// backend for `sovereign service install/remove/status/restart`.
// Non-Ubuntu platforms return a clear error gate.

pub mod adapters;
pub mod apt;
pub mod native;
pub mod systemd;
