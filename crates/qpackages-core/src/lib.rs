//! Reading and planning for the package sources qpackages manages.
//!
//! Nothing here draws anything or asks for privileges. Every source is read through a
//! machine-readable path rather than the output it prints for people, because that output is
//! translated: `pacman -Qi` in a Turkish locale renders `None` as `Hiçbiri` and widens its
//! label column, which no parser can follow.

pub mod backup;
pub mod build;
pub mod catalog;
pub mod check;
pub mod depends;
pub mod flatpak;
pub mod helper;
pub mod lock;
pub mod news;
pub mod pacman;
pub mod reflector;
pub mod review;

pub mod sources;
