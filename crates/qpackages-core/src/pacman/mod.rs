//! Reading pacman's own files and the machine-readable output of its commands.

pub mod command;
mod desc;
pub mod files;
mod localdb;
mod orphans;
mod pacnew;
mod plan;
mod restart;
pub mod syncdb;
mod updates;
mod vercmp;

pub use desc::{Package, parse_desc};
pub use localdb::{Problem, read_local_db};
pub use orphans::read_orphans;
pub use pacnew::pacnew_files;
pub use plan::{Plan, Step, parse_install_plan, parse_remove_plan};
pub use restart::{RESTART, restart_needed};
pub use updates::{Update, UpdateCheck, parse_foreign, parse_updates, read_update_check};
pub use vercmp::vercmp;
