//! The packages whose update the running system only picks up after a restart.

/// Packages that the running system keeps using in their old version until the computer
/// restarts: the kernels and their modules, the processor microcode, the C library and the init
/// system every process shares, the message bus, and the graphics drivers loaded into the kernel.
/// A fixed list, because nothing in pacman's output tells a restart apart from a service reload.
pub const RESTART: [&str; 20] = [
    "linux",
    "linux-lts",
    "linux-zen",
    "linux-hardened",
    "linux-rt",
    "linux-rt-lts",
    "linux-firmware",
    "amd-ucode",
    "intel-ucode",
    "glibc",
    "systemd",
    "systemd-libs",
    "dbus",
    "dbus-broker",
    "nvidia",
    "nvidia-open",
    "nvidia-lts",
    "nvidia-dkms",
    "nvidia-open-dkms",
    "nvidia-utils",
];

/// The names among `updated` that need a restart to take effect, in their order.
#[must_use]
pub fn restart_needed<'a>(updated: impl IntoIterator<Item = &'a str>) -> Vec<&'a str> {
    updated.into_iter().filter(|name| RESTART.contains(name)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_listed_packages_ask_for_a_restart() {
        let updated = ["firefox", "linux", "linux-headers", "glibc", "lib32-glibc", "nvidia-utils", "systemd-ui"];
        assert_eq!(restart_needed(updated), ["linux", "glibc", "nvidia-utils"]);
        assert!(restart_needed(["vlc", "gimp"]).is_empty());
    }
}
