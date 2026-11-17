//! The device adapters (ADR 0012).
//!
//! Each directory under `adapters/` is one adapter; `build.rs` collects
//! them, so adding one changes no other file.
//!
//! This crate is where the limits on an adapter are set, by the compiler and
//! not by reading the source:
//!
//! * it is `no_std`: `std::fs`, `std::net`, `std::process` and the rest of
//!   the standard library are not in scope;
//! * its only dependency is `istesso-device-api`: the framework's insides
//!   and every other crate cannot be named;
//! * unsafe code is forbidden.
//!
//! What is left open is `extern crate std;`, which the language allows in
//! any crate. A test of the framework looks for that word in the adapters'
//! sources. That test is a help for review. An adapter is compiled-in code
//! and is trusted like the rest of the program; nothing here is a sandbox.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

include!(concat!(env!("OUT_DIR"), "/adapters.rs"));
