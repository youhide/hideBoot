//! hideBoot: lists the UKIs in `\EFI\Linux`, boots the newest one with
//! attempts left, counting the attempt by renaming its file first, and
//! falls back to the next when the firmware will not start one. A menu
//! when a key is held at startup, and never otherwise. See the README for
//! the contract, and hideboot-core for the decisions.
//!
//! The OS learns which file it booted from `LoaderBootCountPath`, as from
//! systemd-boot, and marks it good by renaming it without its counter.

#![cfg_attr(target_os = "uefi", no_std)]
#![cfg_attr(target_os = "uefi", no_main)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
// unsafe only in `sys`, which allows it for itself.
#![deny(unsafe_code)]

#[cfg(target_os = "uefi")]
mod app;
#[cfg(target_os = "uefi")]
mod sys;

#[cfg(not(target_os = "uefi"))]
fn main() {
    eprintln!("hideboot is a UEFI application: build it with --target x86_64-unknown-uefi");
    std::process::exit(1);
}
