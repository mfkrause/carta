//! Load YAML options at their command-line position.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use carta::ast::MetaValue;
use clap::{ArgAction, FromArgMatches};
use serde_json::Value;

use super::{
    Cli, cli_command, datadir, parse_metadata_into, template_variables_into, variable_value,
};

#[derive(Default)]
struct Defaults {
    arguments: Vec<OsString>,
    inputs: Vec<PathBuf>,
    metadata: BTreeMap<String, MetaValue>,
    variables: BTreeMap<String, carta_core::template::Value>,
    data_dir: Option<PathBuf>,
    active: Vec<PathBuf>,
    files: usize,
}

pub(super) enum ParseError {
    Argument(clap::Error),
    Defaults(String, u8),
}

impl From<clap::Error> for ParseError {
    fn from(error: clap::Error) -> Self {
        Self::Argument(error)
    }
}

impl ParseError {
    pub(super) fn exit(self) -> ! {
        match self {
            Self::Argument(error) => error.exit(),
            Self::Defaults(message, code) => {
                eprintln!("carta: {message}");
                std::process::exit(i32::from(code));
            }
        }
    }
}

pub(super) fn parse() -> Result<Cli, ParseError> {
    parse_arguments(std::env::args_os().skip(1))
}

fn parse_arguments(arguments: impl IntoIterator<Item = OsString>) -> Result<Cli, ParseError> {
    let mut defaults = Defaults::default();
    let mut pending = Vec::new();
    let mut arguments = normalize_optional_boolean_arguments(arguments)?.into_iter();
    let command = cli_command();
    while let Some(argument) = arguments.next() {
        if argument == "--" {
            pending.push(argument);
            pending.extend(arguments);
            break;
        }
        let Some(text) = argument.to_str() else {
            pending.push(argument);
            continue;
        };
        let mut option = None;
        if let Some(long) = text.strip_prefix("--") {
            let (name, value) = long.split_once('=').map_or((long, None), |(name, value)| {
                (name, Some(OsString::from(value)))
            });
            option = command
                .get_arguments()
                .find(|option| {
                    option.get_long() == Some(name)
                        || option
                            .get_all_aliases()
                            .is_some_and(|aliases| aliases.contains(&name))
                })
                .map(|option| (option, value, None));
        } else if let Some(short) = text.strip_prefix('-') {
            for (position, name) in short.char_indices() {
                let Some(found) = command
                    .get_arguments()
                    .find(|option| option.get_short() == Some(name))
                else {
                    break;
                };
                if takes_value(found) {
                    let value = short
                        .get(position + name.len_utf8()..)
                        .filter(|value| !value.is_empty());
                    let prefix = short.get(..position).filter(|prefix| !prefix.is_empty());
                    option = Some((
                        found,
                        value.map(|value| OsString::from(value.strip_prefix('=').unwrap_or(value))),
                        prefix,
                    ));
                    break;
                }
            }
        }
        if let Some((option, value, prefix)) = option {
            if option.get_id() == "defaults" {
                if let Some(prefix) = prefix {
                    pending.push(format!("-{prefix}").into());
                }
                let value = value.or_else(|| arguments.next()).ok_or_else(|| {
                    cli_command().error(
                        clap::error::ErrorKind::InvalidValue,
                        "--defaults needs a filename",
                    )
                })?;
                defaults.flush(&mut pending)?;
                defaults.load(Path::new(&value))?;
                continue;
            }
            if takes_value(option) && value.is_none() {
                pending.push(argument);
                if let Some(value) = arguments.next() {
                    pending.push(value);
                }
                continue;
            }
        }
        pending.push(argument);
    }
    defaults.flush(&mut pending)?;
    let matches = cli_command()
        .try_get_matches_from(std::iter::once(OsString::from("carta")).chain(defaults.arguments))?;
    let mut cli = Cli::from_arg_matches(&matches)?;
    cli.specified_options = matches
        .ids()
        .filter(|id| {
            matches.value_source(id.as_str()) == Some(clap::parser::ValueSource::CommandLine)
        })
        .map(ToString::to_string)
        .collect();
    cli.template_variables = defaults.variables;
    cli.metadata_values = defaults.metadata;
    defaults.inputs.append(&mut cli.input);
    cli.input = defaults.inputs;
    Ok(cli)
}

fn normalize_optional_boolean_arguments(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<Vec<OsString>, clap::Error> {
    let command = cli_command();
    let mut arguments = arguments.into_iter();
    let mut normalized = Vec::new();
    let mut options_enabled = true;
    while let Some(argument) = arguments.next() {
        if argument == "--" {
            options_enabled = false;
            normalized.push(argument);
            continue;
        }
        let Some(text) = argument.to_str().filter(|_| options_enabled) else {
            normalized.push(argument);
            continue;
        };
        if let Some(long) = text.strip_prefix("--") {
            let (name, value) = long
                .split_once('=')
                .map_or((long, None), |(name, value)| (name, Some(value)));
            if let Some(option) = long_option(&command, name) {
                if is_optional_boolean(option)
                    && let Some(value) = value.and_then(boolean_value)
                {
                    normalized
                        .push(format!("--{}={value}", option.get_long().unwrap_or(name)).into());
                    continue;
                }
                let consumes_next = value.is_none() && takes_value(option);
                normalized.push(argument);
                if consumes_next && let Some(value) = arguments.next() {
                    normalized.push(value);
                }
                continue;
            }
        }

        if let Some(shorts) = text
            .strip_prefix('-')
            .filter(|value| !value.starts_with('-'))
        {
            if let Some(short) = shorts.chars().next()
                && let Some(option) = short_option(&command, short)
            {
                let suffix = &shorts[short.len_utf8()..];
                if is_optional_boolean(option) {
                    if suffix.starts_with('=') {
                        return Err(command.clone().error(
                            clap::error::ErrorKind::InvalidValue,
                            format!("short option '-{short}' does not accept '=VALUE'"),
                        ));
                    }
                    if !suffix.is_empty()
                        && let Some(value) = boolean_value(suffix)
                    {
                        let Some(long) = option.get_long() else {
                            normalized.push(argument);
                            continue;
                        };
                        normalized.push(format!("--{long}={value}").into());
                        continue;
                    }
                }
            }
            let consumes_next = short_option_consumes_next(shorts, &command);
            normalized.push(argument);
            if consumes_next && let Some(value) = arguments.next() {
                normalized.push(value);
            }
            continue;
        }

        normalized.push(argument);
    }
    Ok(normalized)
}

fn long_option<'a>(command: &'a clap::Command, name: &str) -> Option<&'a clap::Arg> {
    command.get_arguments().find(|option| {
        option.get_long() == Some(name)
            || option
                .get_all_aliases()
                .is_some_and(|aliases| aliases.contains(&name))
    })
}

fn short_option(command: &clap::Command, name: char) -> Option<&clap::Arg> {
    command
        .get_arguments()
        .find(|option| option.get_short() == Some(name))
}

fn is_optional_boolean(option: &clap::Arg) -> bool {
    matches!(option.get_action(), ArgAction::Set)
        && option.is_require_equals_set()
        && option.get_value_parser().type_id() == std::any::TypeId::of::<bool>()
}

fn boolean_value(value: &str) -> Option<&'static str> {
    if value.eq_ignore_ascii_case("true") {
        Some("true")
    } else if value.eq_ignore_ascii_case("false") {
        Some("false")
    } else {
        None
    }
}

fn short_option_consumes_next(shorts: &str, command: &clap::Command) -> bool {
    let mut remaining = shorts;
    while let Some(name) = remaining.chars().next() {
        let suffix = &remaining[name.len_utf8()..];
        let Some(option) = short_option(command, name) else {
            return false;
        };
        if is_optional_boolean(option) {
            if boolean_value(suffix).is_some() {
                return false;
            }
        } else if takes_value(option) {
            return suffix.is_empty();
        }
        remaining = suffix;
    }
    false
}

fn takes_value(argument: &clap::Arg) -> bool {
    !argument.is_require_equals_set()
        && matches!(argument.get_action(), ArgAction::Set | ArgAction::Append)
        && argument
            .get_num_args()
            .is_none_or(|range| range.min_values() > 0)
}

impl Defaults {
    fn flush(&mut self, pending: &mut Vec<OsString>) -> Result<(), clap::Error> {
        if pending.is_empty() {
            return Ok(());
        }
        let matches = cli_command().try_get_matches_from(
            std::iter::once(OsString::from("carta")).chain(pending.iter().cloned()),
        )?;
        let cli = Cli::from_arg_matches(&matches)?;
        parse_metadata_into(&mut self.metadata, &cli.metadata);
        template_variables_into(&mut self.variables, &cli, &matches);
        if let Some(directory) = cli.data_dir {
            self.data_dir = Some(directory);
        }
        self.arguments.append(pending);
        Ok(())
    }

    fn load(&mut self, requested: &Path) -> Result<(), ParseError> {
        self.files += 1;
        if self.files > 1024 || self.active.len() >= 64 {
            return Err(ParseError::Defaults(
                "defaults expansion exceeds its file limit".to_owned(),
                64,
            ));
        }
        let mut path = requested.to_path_buf();
        if path.extension().is_none() {
            path.set_extension("yaml");
        }
        if !path.is_file()
            && let Some(directory) = datadir::resolve(self.data_dir.as_deref())
        {
            let candidate = directory.join("defaults").join(&path);
            if candidate.is_file() {
                path = candidate;
            }
        }
        let identity = path
            .canonicalize()
            .map_err(|error| ParseError::Defaults(format!("{}: {error}", path.display()), 1))?;
        if self.active.contains(&identity) {
            return Err(ParseError::Defaults(
                format!("circular defaults file reference in '{}'", path.display()),
                63,
            ));
        }
        let source = std::fs::read_to_string(&path)
            .map_err(|error| ParseError::Defaults(format!("{}: {error}", path.display()), 1))?;
        let invalid = |error| {
            ParseError::Defaults(
                format!("invalid defaults file '{}': {error}", path.display()),
                64,
            )
        };
        let value =
            carta_readers::parse_yaml_value(&source).map_err(|error| invalid(error.to_string()))?;
        let Value::Object(mut fields) = value else {
            return Err(ParseError::Defaults(
                format!("defaults file '{}' must contain a mapping", path.display()),
                64,
            ));
        };
        let directory = path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        if let Some(value) = fields.get("data-dir") {
            self.data_dir = Some(PathBuf::from(
                path_value(value, directory).map_err(&invalid)?,
            ));
        }
        self.active.push(identity);
        if let Some(parents) = fields.remove("defaults") {
            for parent in items(&parents) {
                self.load(Path::new(&path_value(parent, directory).map_err(&invalid)?))?;
            }
        }
        for (key, value) in fields {
            self.apply(&key, value, directory)
                .map_err(|error| invalid(format!("{key}: {error}")))?;
        }
        self.active.pop();
        Ok(())
    }

    fn apply(&mut self, key: &str, value: Value, directory: &Path) -> Result<(), String> {
        if matches!(
            key,
            "input-files"
                | "metadata-files"
                | "pdf-engine-opts"
                | "syntax-definitions"
                | "epub-fonts"
                | "filters"
                | "resource-path"
        ) && !value.is_array()
        {
            return Err("expected a list".to_owned());
        }
        match key {
            "input-files" | "input-file" => {
                for value in items(&value) {
                    self.inputs
                        .push(PathBuf::from(path_value(value, directory)?));
                }
            }
            "metadata" => {
                let Value::Object(values) = value else {
                    return Err("expected a mapping".to_owned());
                };
                self.metadata.extend(
                    values
                        .into_iter()
                        .map(|(key, value)| (key, metadata_value(value))),
                );
            }
            "variables" => {
                let Value::Object(values) = value else {
                    return Err("expected a mapping".to_owned());
                };
                self.variables.extend(
                    values
                        .into_iter()
                        .map(|(key, value)| (key, variable_value(value))),
                );
            }
            "html-math-method" => self.math_method(&value)?,
            "filters" => {
                for value in items(&value) {
                    let path = if let Value::Object(filter) = value {
                        let kind = filter.get("type").and_then(Value::as_str).unwrap_or("json");
                        if kind != "json" {
                            return Err(format!("unsupported filter type '{kind}'"));
                        }
                        filter
                            .get("path")
                            .ok_or_else(|| "filter needs a path".to_owned())?
                    } else {
                        value
                    };
                    self.option("filter", path_value(path, directory)?);
                }
            }
            "resource-path" => {
                let paths: Result<Vec<_>, _> = items(&value)
                    .map(|value| path_value(value, directory).map(PathBuf::from))
                    .collect();
                let paths = std::env::join_paths(paths?).map_err(|error| error.to_string())?;
                self.option(key, paths);
            }
            _ => self.apply_option(key, &value, directory)?,
        }
        Ok(())
    }

    fn apply_option(&mut self, key: &str, value: &Value, directory: &Path) -> Result<(), String> {
        if matches!(
            key,
            "output"
                | "variable"
                | "variable-json"
                | "filter"
                | "stylesheet"
                | "no-highlight"
                | "mathjax"
                | "katex"
                | "mathml"
                | "webtex"
        ) || key.starts_with("list-")
            || key.starts_with("print-")
        {
            return Err(format!("unknown option '{key}'"));
        }
        let key = match key {
            "reader" => "from",
            "writer" => "to",
            "output-file" => "output",
            "metadata-files" => "metadata-file",
            "pdf-engine-opts" => "pdf-engine-opt",
            "syntax-definitions" => "syntax-definition",
            "epub-fonts" => "epub-embed-font",
            _ => key,
        };
        let command = cli_command();
        let argument = command
            .get_arguments()
            .find(|argument| {
                argument.get_long() == Some(key)
                    || argument
                        .get_all_aliases()
                        .is_some_and(|aliases| aliases.contains(&key))
            })
            .filter(|argument| {
                !argument.is_hide_set()
                    && !matches!(argument.get_action(), ArgAction::Help | ArgAction::Version)
            })
            .ok_or_else(|| format!("unknown option '{key}'"))?;
        let boolean = argument.get_value_parser().type_id() == std::any::TypeId::of::<bool>();
        if value.is_array() && !matches!(argument.get_action(), ArgAction::Append) {
            return Err("expected a single value".to_owned());
        }
        for value in items(value) {
            let text = if boolean {
                value
                    .as_bool()
                    .ok_or_else(|| "expected a boolean".to_owned())?
                    .to_string()
                    .into()
            } else if argument.get_value_parser().type_id() == std::any::TypeId::of::<PathBuf>() {
                path_value(value, directory)?
            } else {
                match value {
                    Value::String(text) => text.into(),
                    Value::Number(number) => number.to_string().into(),
                    _ => return Err("expected text or a number".to_owned()),
                }
            };
            if key == "data-dir" {
                self.data_dir = Some(PathBuf::from(&text));
            }
            self.option(key, text);
        }
        Ok(())
    }

    fn math_method(&mut self, value: &Value) -> Result<(), String> {
        let (method, url) = match value {
            Value::String(method) => (method.as_str(), None),
            Value::Object(fields) => (
                fields
                    .get("method")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "math method needs a name".to_owned())?,
                fields
                    .get("url")
                    .map(|value| {
                        value
                            .as_str()
                            .ok_or_else(|| "math URL must be text".to_owned())
                    })
                    .transpose()?,
            ),
            _ => return Err("expected a math method name or mapping".to_owned()),
        };
        match method {
            "mathjax" | "katex" | "webtex" => {
                if let Some(url) = url {
                    self.option(method, url.into());
                } else {
                    self.arguments.push(format!("--{method}").into());
                }
            }
            "mathml" => self.arguments.push("--mathml".into()),
            "plain" => self.arguments.push("--mathml=false".into()),
            _ => return Err(format!("unsupported math method '{method}'")),
        }
        Ok(())
    }

    fn option(&mut self, name: &str, value: OsString) {
        let mut argument = OsString::from(format!("--{name}="));
        argument.push(value);
        self.arguments.push(argument);
    }
}

fn items(value: &Value) -> std::slice::Iter<'_, Value> {
    value
        .as_array()
        .map_or_else(|| std::slice::from_ref(value), Vec::as_slice)
        .iter()
}

fn path_value(value: &Value, directory: &Path) -> Result<OsString, String> {
    let mut text = value
        .as_str()
        .ok_or_else(|| "expected a filename".to_owned())?;
    let mut result = OsString::new();
    while let Some((before, after)) = text.split_once("${") {
        let Some((name, rest)) = after.split_once('}') else {
            break;
        };
        result.push(before);
        if name == "." {
            result.push(directory);
        } else if let Some(value) = std::env::var_os(name) {
            result.push(value);
        } else {
            eprintln!("carta: undefined environment variable '{name}' in defaults file");
        }
        text = rest;
    }
    result.push(OsStr::new(text));
    Ok(result)
}

fn metadata_value(value: Value) -> MetaValue {
    match value {
        Value::Null => MetaValue::MetaString("".into()),
        Value::Bool(value) => MetaValue::MetaBool(value),
        Value::String(value) => MetaValue::MetaString(value.into()),
        Value::Number(value) => MetaValue::MetaString(value.to_string().into()),
        Value::Array(values) => {
            MetaValue::MetaList(values.into_iter().map(metadata_value).collect())
        }
        Value::Object(values) => MetaValue::MetaMap(
            values
                .into_iter()
                .map(|(key, value)| (key.into(), metadata_value(value)))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic, clippy::unwrap_used)]

    use super::{ParseError, parse_arguments};
    use crate::Cli;
    use std::ffi::OsString;
    use std::path::PathBuf;

    fn parse_cli(arguments: &[&str]) -> Cli {
        match parse_arguments(arguments.iter().map(|argument| OsString::from(*argument))) {
            Ok(cli) => cli,
            Err(ParseError::Argument(error)) => panic!("{error}"),
            Err(ParseError::Defaults(message, _)) => panic!("{message}"),
        }
    }

    #[test]
    fn preserve_tabs_accepts_pandoc_long_and_short_forms() {
        for (arguments, expected) in [
            (&["-p"][..], true),
            (&["-ptrue"][..], true),
            (&["-pTRUE"][..], true),
            (&["-pfalse"][..], false),
            (&["-pFalse"][..], false),
            (&["--preserve-tabs=TRUE"][..], true),
            (&["--preserve-tabs=False"][..], false),
            (&["-pfalse", "-ptrue"][..], true),
            (
                &["--preserve-tabs=true", "--preserve-tabs=FALSE"][..],
                false,
            ),
        ] {
            let cli = parse_cli(arguments);
            assert_eq!(cli.preserve_tabs, expected, "{arguments:?}");
        }
    }

    #[test]
    fn optional_boolean_forms_match_pandoc_for_standalone() {
        for (arguments, expected) in [
            (&["-sfalse"][..], false),
            (&["--standalone=TRUE"][..], true),
            (&["--standalone=FALSE"][..], false),
        ] {
            assert_eq!(parse_cli(arguments).standalone, expected, "{arguments:?}");
        }
        assert!(parse_arguments([OsString::from("-s=false")]).is_err());
    }

    #[test]
    fn short_boolean_groups_keep_clap_parsing() {
        for argument in ["-spfalse", "-psfalse"] {
            assert_eq!(parse_cli(&[argument]).from.as_deref(), Some("alse"));
        }
    }

    #[test]
    fn preserve_tabs_without_equals_does_not_consume_the_next_input() {
        for arguments in [&["--preserve-tabs", "false"][..], &["-p", "false"][..]] {
            let cli = parse_cli(arguments);
            assert!(cli.preserve_tabs, "{arguments:?}");
            assert_eq!(cli.input, [PathBuf::from("false")], "{arguments:?}");
        }

        let cli = parse_cli(&["--", "-pfalse"]);
        assert!(!cli.preserve_tabs);
        assert_eq!(cli.input, [PathBuf::from("-pfalse")]);
    }

    #[test]
    fn required_option_values_that_look_like_options_stay_values() {
        for value in ["-pfalse", "--standalone=TRUE"] {
            let cli = parse_cli(&["--pdf-engine-opt", value]);
            assert_eq!(cli.pdf_engine_options, [value]);
            assert!(!cli.preserve_tabs);
            assert!(!cli.standalone);
        }
    }

    #[test]
    fn preserve_tabs_rejects_pandoc_invalid_values_and_short_equals() {
        for argument in [
            "-p=false",
            "--preserve-tabs=yes",
            "--preserve-tabs=0",
            "--preserve-tabs=1",
            "--preserve-tabs=",
        ] {
            assert!(
                parse_arguments([OsString::from(argument)]).is_err(),
                "{argument:?}"
            );
        }

        for arguments in [&["-p", "--tab-stop=0"][..], &["--tab-stop=0", "-p"][..]] {
            assert!(
                parse_arguments(arguments.iter().map(|argument| OsString::from(*argument)))
                    .is_err()
            );
        }
    }
}
