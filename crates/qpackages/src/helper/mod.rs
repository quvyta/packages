//! The root helper: one narrow process per run that carries out every transaction.
//!
//! [`root`] is the helper itself, qpac started as root; [`session`] owns it from the user's
//! side, and [`pkexec`] connects one that polkit let start. The protocol they speak lives in
//! [`qpackages_core::helper`].

pub mod pkexec;
pub mod root;
pub mod session;
