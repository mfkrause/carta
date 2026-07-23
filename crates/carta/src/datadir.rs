//! The data directory: a per-user location holding overrides for filters (`filters/`) and templates
//! (`templates/`). It is chosen by the `--data-dir` flag, falling back to the platform's conventional
//! per-user data location.

use std::path::{Path, PathBuf};

/// The effective data directory: the explicit `--data-dir` when given, otherwise the per-user data
/// location: `$XDG_DATA_HOME/carta`, or `$HOME/.local/share/carta` when that variable is unset.
///
/// Returns `None` only when no directory can be determined (no flag and no home location), in which
/// case lookups against it are skipped. A returned path is not required to exist: callers test
/// for the specific file they want and fall through when it is absent.
pub(crate) fn resolve(explicit: Option<&Path>) -> Option<PathBuf> {
    resolve_from(
        explicit,
        non_empty_var("XDG_DATA_HOME"),
        non_empty_var("HOME"),
    )
}

/// The resolution itself, taking the environment values as arguments so it can be exercised without
/// touching the process environment.
fn resolve_from(
    explicit: Option<&Path>,
    xdg_data_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    if let Some(dir) = explicit {
        return Some(dir.to_path_buf());
    }
    if let Some(base) = xdg_data_home {
        return Some(Path::new(&base).join("carta"));
    }
    home.map(|home| Path::new(&home).join(".local").join("share").join("carta"))
}

/// The value of environment variable `name`, treating an empty value as unset.
fn non_empty_var(name: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::resolve_from;
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};

    fn os(value: &str) -> OsString {
        OsString::from(value)
    }

    #[test]
    fn explicit_directory_wins_over_the_environment() {
        let chosen = resolve_from(
            Some(Path::new("/custom/dir")),
            Some(os("/xdg")),
            Some(os("/home/user")),
        );
        assert_eq!(chosen, Some(PathBuf::from("/custom/dir")));
    }

    #[test]
    fn xdg_data_home_is_suffixed_with_the_app_name() {
        let chosen = resolve_from(None, Some(os("/xdg")), Some(os("/home/user")));
        assert_eq!(chosen, Some(PathBuf::from("/xdg/carta")));
    }

    #[test]
    fn falls_back_to_the_home_data_location() {
        let chosen = resolve_from(None, None, Some(os("/home/user")));
        assert_eq!(chosen, Some(PathBuf::from("/home/user/.local/share/carta")));
    }

    #[test]
    fn no_directory_when_nothing_is_available() {
        assert_eq!(resolve_from(None, None, None), None);
    }
}
