//! Building AUR packages with paru or yay while their root steps go through the helper.
//!
//! paru and yay build as the user and ask a sudo-like program for every step that needs root.
//! qpac hands them its own path as that program (`--sudo`), with [`shim::FLAG`] and a private
//! folder as its first arguments (`--sudoflags`). Started that way, qpac is the shim: it reads the
//! pacman call it was given, turns it into one helper request, writes that to the folder's
//! `request` pipe and relays the answer from its `answer` pipe. The qpac that started the build
//! reads the request, checks it once more against what this build is expected to ask for
//! ([`Expected`]) and passes it to its helper.
//!
//! - [`shim`] reads the pacman calls paru and yay make, a closed set measured on both;
//! - [`Expected`] is the second check, by the build's own package lists;
//! - [`command`] has the paru and yay command lines;
//! - [`plan`] works out what a build installs before the user confirms it.

pub mod command;
mod expected;
pub mod plan;
pub mod shim;

pub use expected::Expected;
pub use plan::{Built, Plan};
