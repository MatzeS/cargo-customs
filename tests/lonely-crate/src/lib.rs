#![cfg_attr(target_os = "none", no_std)]

#[cfg(target_os = "none")]
pub mod only_embedded;

#[cfg(not(target_os = "none"))]
pub mod only_host;

pub fn foo() {}
