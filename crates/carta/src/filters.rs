//! JSON filters: transform the document by piping it, as JSON, through external programs.
//!
//! Each filter is a program that reads the document as JSON on standard input and writes the
//! transformed document as JSON on standard output, receiving the output format name as its sole
//! argument. Filters apply in order, each one seeing the previous one's result.
//!
//! A bare filter name (one with no path component) is resolved against the data directory's
//! `filters/` first, then the working directory, then the executable search path. A resolved file
//! that lacks the executable bit is run through an interpreter chosen from its extension, so a
//! `filter.py` runs even without a shebang or `+x`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use carta::{Document, Error, Result};

/// Runs `document` through each filter in turn. `format` is the output format name passed to every
/// filter as its argument; `data_dir`, when set, has its `filters/` subdirectory searched before the
/// working directory and executable path.
///
/// # Errors
/// [`Error::Filter`] if a filter cannot be launched, exits unsuccessfully, or emits output that is
/// not a valid document.
pub(crate) fn run(
    document: &mut Document,
    filters: &[String],
    format: &str,
    data_dir: Option<&Path>,
) -> Result<()> {
    for filter in filters {
        run_one(document, filter, format, data_dir)?;
    }
    Ok(())
}

fn run_one(
    document: &mut Document,
    filter: &str,
    format: &str,
    data_dir: Option<&Path>,
) -> Result<()> {
    let input = carta::ast::to_json(document)?;

    let mut command = command_for(filter, data_dir);
    command
        .arg(format)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let mut child = command
        .spawn()
        .map_err(|error| Error::Filter(format!("could not run filter '{filter}': {error}")))?;

    // Feed the JSON on a separate thread while draining stdout on this one: a filter that emits a
    // large document fills its output pipe and blocks on the write before it finishes reading its
    // input, which would deadlock a single thread that writes the whole input before it starts
    // reading.
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| Error::Filter(format!("filter '{filter}' provides no standard input")))?;
    let feeder = std::thread::spawn(move || stdin.write_all(input.as_bytes()));

    let output = child
        .wait_with_output()
        .map_err(|error| Error::Filter(format!("filter '{filter}' could not be run: {error}")))?;
    let fed = feeder.join();

    if !output.status.success() {
        return Err(Error::Filter(format!(
            "filter '{filter}' failed ({})",
            output.status
        )));
    }
    // A write failure matters only when the filter itself reported success: a filter that closes its
    // input early, having read all it needs, leaves the feeder with a broken pipe, which is expected.
    if let Ok(Err(error)) = fed
        && error.kind() != std::io::ErrorKind::BrokenPipe
    {
        return Err(Error::Filter(format!(
            "could not send the document to filter '{filter}': {error}"
        )));
    }

    *document = carta::ast::from_json(&output.stdout).map_err(|error| {
        Error::Filter(format!(
            "filter '{filter}' produced invalid output: {error}"
        ))
    })?;
    Ok(())
}

/// Builds the command that runs `filter`, resolving its location and choosing a direct invocation or
/// an interpreter as appropriate.
fn command_for(filter: &str, data_dir: Option<&Path>) -> Command {
    match resolve(filter, data_dir) {
        Target::File(path) => command_for_file(&path),
        Target::Search(name) => Command::new(name),
    }
}

/// What a filter name resolves to.
enum Target {
    /// A file on disk: run it directly when executable, else through an interpreter chosen from its
    /// extension.
    File(PathBuf),
    /// A bare program name to look up on the executable search path.
    Search(String),
}

fn resolve(filter: &str, data_dir: Option<&Path>) -> Target {
    // A name with a path component addresses a file directly. A bare name is looked up in the data
    // directory's `filters/`, then the working directory, and finally left for the executable search
    // path.
    if has_separator(filter) {
        return Target::File(PathBuf::from(filter));
    }
    if let Some(dir) = data_dir {
        let candidate = dir.join("filters").join(filter);
        if candidate.is_file() {
            return Target::File(candidate);
        }
    }
    if Path::new(filter).is_file() {
        // Give the command an explicit relative path so it runs the working-directory file rather
        // than searching the executable path for the bare name.
        return Target::File(Path::new(".").join(filter));
    }
    Target::Search(filter.to_owned())
}

fn command_for_file(path: &Path) -> Command {
    if is_executable(path) {
        return Command::new(path);
    }
    if let Some(interpreter) = interpreter_for(path) {
        let mut command = Command::new(interpreter);
        command.arg(path);
        return command;
    }
    // Not executable and no known interpreter: run it directly and let the OS report why it cannot.
    Command::new(path)
}

fn has_separator(name: &str) -> bool {
    name.contains('/')
        || (std::path::MAIN_SEPARATOR != '/' && name.contains(std::path::MAIN_SEPARATOR))
}

/// The interpreter that runs a script of the given file extension, when it has no executable bit.
fn interpreter_for(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match extension.as_str() {
        "py" => "python",
        "js" => "node",
        "rb" => "ruby",
        "php" => "php",
        "pl" => "perl",
        "hs" => "runhaskell",
        "r" => "Rscript",
        _ => return None,
    })
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|meta| meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    // Without a Unix executable bit, defer to the OS loader, which keys on the extension (`.exe`, …).
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::{Target, has_separator, interpreter_for, resolve};
    use std::path::{Path, PathBuf};

    #[test]
    fn interpreter_is_chosen_from_the_extension() {
        assert_eq!(interpreter_for(Path::new("f.py")), Some("python"));
        assert_eq!(interpreter_for(Path::new("f.js")), Some("node"));
        assert_eq!(interpreter_for(Path::new("f.rb")), Some("ruby"));
        assert_eq!(interpreter_for(Path::new("f.php")), Some("php"));
        assert_eq!(interpreter_for(Path::new("f.pl")), Some("perl"));
        assert_eq!(interpreter_for(Path::new("f.hs")), Some("runhaskell"));
        assert_eq!(interpreter_for(Path::new("f.r")), Some("Rscript"));
    }

    #[test]
    fn interpreter_extension_match_ignores_case() {
        assert_eq!(interpreter_for(Path::new("Filter.PY")), Some("python"));
        assert_eq!(interpreter_for(Path::new("stats.R")), Some("Rscript"));
    }

    #[test]
    fn unknown_or_absent_extension_has_no_interpreter() {
        assert_eq!(interpreter_for(Path::new("filter.sh")), None);
        assert_eq!(interpreter_for(Path::new("filter")), None);
    }

    #[test]
    fn a_path_component_marks_a_direct_file() {
        assert!(has_separator("./filter"));
        assert!(has_separator("bin/filter"));
        assert!(has_separator("/abs/filter"));
        assert!(!has_separator("filter"));
        assert!(!has_separator("cite-filter"));
    }

    #[test]
    fn a_name_with_a_path_addresses_a_file_directly() {
        // A separator bypasses the data directory and working-directory search entirely.
        match resolve("./nested/f", Some(Path::new("/data"))) {
            Target::File(path) => assert_eq!(path, PathBuf::from("./nested/f")),
            Target::Search(_) => panic!("a path should resolve to a file"),
        }
    }

    #[test]
    fn a_bare_name_falls_through_to_the_search_path() {
        // With no data directory and no such file in the working directory, a bare name is left for
        // the executable search path.
        let missing = Path::new("/does/not/exist");
        match resolve("cite-filter", Some(missing)) {
            Target::Search(name) => assert_eq!(name, "cite-filter"),
            Target::File(_) => panic!("a bare unresolved name should be searched"),
        }
    }

    /// A fresh, empty scratch directory unique to `name`, under the system temp location.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_bare_name_resolves_within_the_data_directory() {
        let dir = scratch("carta-filter-datadir-resolve");
        let filters = dir.join("filters");
        std::fs::create_dir_all(&filters).unwrap();
        let script = filters.join("myfilter");
        std::fs::write(&script, "#!/bin/sh\ncat\n").unwrap();

        match resolve("myfilter", Some(&dir)) {
            Target::File(path) => assert_eq!(path, script),
            Target::Search(_) => panic!("the data-directory filter should be found"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_executable_bit_is_read_from_the_file_mode() {
        use super::is_executable;
        use std::os::unix::fs::PermissionsExt;

        let dir = scratch("carta-filter-exec-bit");
        let file = dir.join("script");
        std::fs::write(&file, "#!/bin/sh\ncat\n").unwrap();

        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!is_executable(&file));
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(is_executable(&file));
    }
}
