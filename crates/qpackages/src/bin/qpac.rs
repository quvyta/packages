//! The `qpac` command, the short name people type.

fn main() -> std::io::Result<()> {
    // The check a user timer starts draws nothing, so it is decided before the screen is set up.
    if std::env::args_os().nth(1).is_some_and(|first| first == quvyta_packages::check::FLAG) {
        std::process::exit(quvyta_packages::check::run());
    }
    quvyta_packages::run()
}
