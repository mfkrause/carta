//! Control-line parsing and macro-argument helpers for the `man` reader.

/// Whether a line is a control line, one introduced by the `.` or `'` control character.
fn is_control(line: &str) -> bool {
    line.starts_with('.') || line.starts_with('\'')
}

/// Whether a control line is a comment (`.\"` or `.\#`).
pub(super) fn is_comment(line: &str) -> bool {
    if !is_control(line) {
        return false;
    }
    let body = line.get(1..).unwrap_or("");
    body.starts_with("\\\"") || body.starts_with("\\#")
}

/// Splits a control line into its request name and the remaining argument text, or returns `None`
/// for a text line. Whitespace between the control character and the request name is allowed and
/// skipped, so `.  SH` names the `SH` request.
pub(super) fn control_parts(line: &str) -> Option<(&str, &str)> {
    if !is_control(line) {
        return None;
    }
    let body = line.get(1..).unwrap_or("").trim_start_matches([' ', '\t']);
    match body.split_once([' ', '\t']) {
        Some((name, rest)) => Some((name, rest.trim_start_matches([' ', '\t']))),
        None => Some((body, "")),
    }
}

/// Whether a request name marks a no-op control line: an empty request (a bare control character) or
/// one named only with control characters (`.`, `..`, `...`, `'`). Such a line is transparent and
/// does not interrupt fill.
pub(super) fn is_noop_request(name: &str) -> bool {
    name.chars().all(|c| matches!(c, '.' | '\''))
}

/// Splits a conditional request's argument into its one-token condition and the branch text that
/// follows it.
pub(super) fn split_condition(rest: &str) -> (&str, &str) {
    match rest.split_once([' ', '\t']) {
        Some((cond, branch)) => (cond, branch),
        None => (rest, ""),
    }
}

/// Evaluates a conditional request's condition. The nroff target (`n`) and the constant `1` are
/// true; every other condition (the troff target `t`, `0`, other numbers, register and string
/// tests) is treated as false.
pub(super) fn condition_true(cond: &str) -> bool {
    cond == "n" || cond == "1"
}

/// Splits a macro argument string the way `groff` does: on spaces and tabs, with double quotes
/// grouping an argument that may contain spaces and `""` denoting a literal quote. A backslash keeps
/// the following character (so an escaped space does not split).
pub(super) fn split_args(input: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut chars = input.chars().peekable();
    loop {
        while matches!(chars.peek(), Some(' ' | '\t')) {
            chars.next();
        }
        if chars.peek().is_none() {
            break;
        }
        let mut arg = String::new();
        if chars.peek() == Some(&'"') {
            chars.next();
            while let Some(c) = chars.next() {
                if c == '"' {
                    if chars.peek() == Some(&'"') {
                        chars.next();
                        arg.push('"');
                    } else {
                        break;
                    }
                } else {
                    arg.push(c);
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c == ' ' || c == '\t' {
                    break;
                }
                chars.next();
                arg.push(c);
                if c == '\\'
                    && let Some(next) = chars.next()
                {
                    arg.push(next);
                }
            }
        }
        args.push(arg);
    }
    args
}

/// Applies copy-mode reduction to a line as it is stored in a macro body: an escaped backslash
/// `\\` collapses to a single `\`. This defers the remaining escapes (argument references `\$N`
/// among them) to the moment the macro is invoked, so a body written with `\\$1` and one written
/// with `\$1` resolve identically when the macro runs.
pub(super) fn reduce_copy_mode(line: &str) -> String {
    if !line.contains('\\') {
        return line.to_owned();
    }
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek() == Some(&'\\') {
            chars.next();
        }
        out.push(c);
    }
    out
}

/// Substitutes a macro call's arguments for `\$N` references in one body line. `\$1`..`\$9` expand to
/// the corresponding argument (an absent one to nothing) and `\$0` to nothing; a doubled backslash
/// before the reference (`\\$N`, how a reference is written so it survives definition-time copying) is
/// treated the same. Every other backslash sequence is left untouched.
pub(super) fn substitute_macro_args(line: &str, args: &[String]) -> String {
    if !line.contains("\\$") {
        return line.to_owned();
    }
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('$') => {
                chars.next();
                push_macro_arg(&mut chars, args, &mut out);
            }
            // Keep `\\` intact; consuming it lets a following `$` read as an argument reference.
            Some('\\') => {
                chars.next();
                out.push('\\');
                out.push('\\');
            }
            _ => out.push('\\'),
        }
    }
    out
}

/// After a `\$` reference, reads the one-digit argument index and appends the corresponding call
/// argument (nothing for `\$0` or an out-of-range index).
fn push_macro_arg(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    args: &[String],
    out: &mut String,
) {
    if let Some(&digit) = chars.peek()
        && let Some(index) = digit.to_digit(10)
    {
        chars.next();
        if index >= 1
            && let Some(arg) = args.get((index - 1) as usize)
        {
            out.push_str(arg);
        }
    }
}
