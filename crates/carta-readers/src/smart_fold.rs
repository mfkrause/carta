//! Smart-typography folds shared by the readers that curl straight punctuation runs.

/// Fold a run of `len` dots into one ellipsis (`…`) per group of three, leaving the remaining one or
/// two dots literal.
pub(crate) fn fold_ellipsis_run(len: usize) -> String {
    let mut out = String::with_capacity(len / 3 * 3 + len % 3);
    out.extend(std::iter::repeat_n('\u{2026}', len / 3));
    out.extend(std::iter::repeat_n('.', len % 3));
    out
}
