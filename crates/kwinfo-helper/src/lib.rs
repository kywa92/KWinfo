//! The privileged helper: reads the sensors that only root may read and hands them to
//! KWinfo over a pipe (`kwinfo-helper stdio`, started through `pkexec` for one session)
//! or over the D-Bus system bus (`kwinfo-helper serve`, started by D-Bus activation once
//! `kwinfo-helper install` or a package has put the service files in place).
//!
//! It never writes to hardware. Every reading comes from a [`providers::Provider`]; the
//! transport-independent request handling is [`server::Helper`]. See PLAN.md 3.5.

#[cfg(unix)]
pub mod dbus;
pub mod install;
pub mod providers;
pub mod server;
pub mod stdio;

pub use server::Helper;
