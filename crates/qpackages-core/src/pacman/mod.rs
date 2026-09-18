//! Reading pacman's own files and the machine-readable output of its commands.

pub mod command;
mod desc;
mod localdb;
mod plan;
pub mod syncdb;
mod updates;

pub use desc::{Package, parse_desc};
pub use localdb::{Problem, read_local_db};
pub use plan::{Plan, Step, parse_install_plan, parse_remove_plan};
pub use updates::{Update, UpdateCheck, parse_updates, read_update_check};
