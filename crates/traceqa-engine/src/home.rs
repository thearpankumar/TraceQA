//! TraceQA's own folder, where `traceqa install` puts the drivers.
//!
//! ```text
//! ~/.traceqa/            or $TRACEQA_HOME
//!   env/                 a Python environment holding the drivers
//!     bin/ or Scripts/   their launchers
//! ```
//!
//! The engine looks here for a default driver that is not on PATH, so a user who ran
//! `traceqa install` needs nothing else.

use std::env;
use std::path::{Path, PathBuf};

/// Environment variable that moves TraceQA's folder.
pub const HOME_ENV: &str = "TRACEQA_HOME";

/// TraceQA's folder: `$TRACEQA_HOME`, else `.traceqa` in the user's home folder.
pub fn dir() -> Option<PathBuf> {
    if let Some(home) = env::var_os(HOME_ENV).filter(|home| !home.is_empty()) {
        return Some(PathBuf::from(home));
    }
    let user = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    env::var_os(user)
        .filter(|home| !home.is_empty())
        .map(|home| PathBuf::from(home).join(".traceqa"))
}

/// The Python environment the drivers are installed into.
pub fn env_dir(home: &Path) -> PathBuf {
    home.join("env")
}

/// Where an environment keeps its programs. Python names the folder differently on Windows.
pub fn bin_dir(env: &Path) -> PathBuf {
    env.join(if cfg!(windows) { "Scripts" } else { "bin" })
}

/// The file name of a program on this system.
pub fn program(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// The installed copy of a program, when `traceqa install` put one there.
pub fn installed(name: &str) -> Option<PathBuf> {
    let path = bin_dir(&env_dir(&dir()?)).join(program(name));
    path.is_file().then_some(path)
}

/// The first program called `name` on PATH. On Windows the usual launcher extensions count too.
pub fn on_path(name: &str) -> Option<PathBuf> {
    let extensions: &[&str] = if cfg!(windows) {
        &["", ".exe", ".cmd", ".bat"]
    } else {
        &[""]
    };
    env::split_paths(&env::var_os("PATH")?)
        .filter(|dir| !dir.as_os_str().is_empty())
        .flat_map(|dir| {
            extensions
                .iter()
                .map(move |extension| dir.join(format!("{name}{extension}")))
        })
        .find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launchers_live_where_python_puts_them() {
        let env = env_dir(Path::new("home"));
        let expected = if cfg!(windows) { "Scripts" } else { "bin" };
        assert_eq!(bin_dir(&env), Path::new("home").join("env").join(expected));
        assert_eq!(program("traceqa-driver-playwright").ends_with(".exe"), cfg!(windows));
    }

    #[test]
    fn a_missing_program_is_not_on_path() {
        assert_eq!(on_path("traceqa-no-such-program-anywhere"), None);
    }
}
