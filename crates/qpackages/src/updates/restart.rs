//! Whether an update only takes full effect after a restart. The list is the core's
//! [`RESTART`](qpackages_core::pacman::RESTART), so the Updates tab and the transaction never
//! disagree; a wrong guess either way would teach the user to ignore the note.

/// Whether updating `name` needs a restart to take full effect.
#[must_use]
pub fn needs_restart(name: &str) -> bool {
    qpackages_core::pacman::RESTART.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kernel_and_the_c_library_need_a_restart_and_an_ordinary_program_does_not() {
        for name in ["linux", "linux-lts", "systemd", "glibc", "nvidia-open"] {
            assert!(needs_restart(name), "{name}");
        }
        for name in ["mesa", "firefox", "linux-api-headers", "glibc-locales"] {
            assert!(!needs_restart(name), "{name}");
        }
    }
}
