//! Building AUR packages: the shim paru and yay call in place of sudo, the pipes it talks
//! through, and the relay that passes its requests to the helper.
//!
//! What is accepted and why lives in [`qpackages_core::build`]; this is the part that opens
//! pipes and runs threads.

pub mod lookup;
pub mod pipes;
pub mod relay;
pub mod shim;

#[cfg(test)]
pub(crate) mod tests;
