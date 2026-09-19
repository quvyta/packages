//! Reading and planning for the package sources qpackages manages.
//!
//! Nothing here draws anything or asks for privileges. Every source is read through a
//! machine-readable path rather than the output it prints for people, because that output is
//! translated: `pacman -Qi` in a Turkish locale renders `None` as `Hiçbiri` and widens its
//! label column, which no parser can follow.

pub mod catalog;
pub mod helper;
pub mod lock;
pub mod pacman;

pub mod sources;
