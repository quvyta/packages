//! The packages whose update only takes full effect after a restart.
//!
//! The list is fixed rather than guessed: the kernels and their firmware and microcode, the init
//! system, the C library every running program has loaded, the system bus, and the graphics
//! drivers that live in the kernel. A wrong guess either way would teach the user to ignore the
//! note.

/// Every package the note is shown for.
const NEEDS_RESTART: [&str; 20] = [
    "linux",
    "linux-lts",
    "linux-zen",
    "linux-hardened",
    "linux-rt",
    "linux-rt-lts",
    "linux-firmware",
    "amd-ucode",
    "intel-ucode",
    "systemd",
    "systemd-libs",
    "glibc",
    "dbus",
    "dbus-broker",
    "nvidia",
    "nvidia-lts",
    "nvidia-open",
    "nvidia-dkms",
    "nvidia-open-dkms",
    "nvidia-utils",
];

/// Whether updating `name` needs a restart to take full effect.
#[must_use]
pub fn needs_restart(name: &str) -> bool {
    NEEDS_RESTART.contains(&name)
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
