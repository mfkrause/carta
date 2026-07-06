# Contributing to carta

Thanks for your interest in improving carta! This guide covers the mechanics of
contributing.

## Getting set up

The Rust toolchain is pinned in [`rust-toolchain.toml`](rust-toolchain.toml); `rustup`
installs the right version automatically. A few extra tools are used by the test and
lint suites:

```sh
cargo install cargo-nextest      # test runner (required)
cargo install cargo-insta        # snapshot review (Layer 1)
cargo install cargo-llvm-cov     # coverage
cargo install cargo-deny         # dependency/license/advisory checks
cargo install typos-cli          # optional — spell check, for CI parity via tools/check.sh
cargo install hyperfine          # optional — used only by tools/bench-suite
```

Run the one-time developer setup to enable the git hooks (formatting on commit; clippy
and tests on push):

```sh
tools/dev-setup.sh
```

## Everyday workflow

```sh
cargo build                                # build the workspace
cargo nextest run --workspace              # run the offline test suite
cargo test --doc --workspace               # doctests
cargo fmt --all                            # format
RUSTFLAGS="-D warnings" cargo clippy --all-targets --all-features  # lint (as CI does)
tools/check.sh                             # everything CI gates a PR on, in one command
```

The everyday suite is fully offline and is what CI gates every pull request on. Broader
conformance and fuzzing layers, along with the tooling they need, are described in
[`AGENTS.md`](AGENTS.md) and the [`docs/`](docs/) directory; they are not required for
most contributions.

### Snapshot tests

Golden output is captured with [`insta`](https://insta.rs). After an intentional change
to output, review and accept the new snapshots — never hand-edit `.snap` files:

```sh
cargo insta review
```

CI rejects stale or unreferenced snapshots, so keep them tidy.

## Making a change

- **Branch off `main`** for anything non-trivial; do not commit directly to `main`.
- **One logical change per commit.** Commit messages follow
  [Conventional Commits](https://www.conventionalcommits.org/) (`feat`, `fix`, `docs`,
  `refactor`, `perf`, `test`, `build`, `ci`, `chore`, …); the `commit-msg` hook enforces
  the format.
- **Keep output deterministic** and **avoid panics in library paths** (both are lint-enforced).
- When you add, extend, or change support for a format or extension, update
  [`README.md`](README.md) (the status table) and/or [`docs/STATUS.md`](docs/STATUS.md) in
  the same change.
- Make sure `cargo fmt`, `cargo clippy`, and the test suite are green before opening a PR.

## Opening a pull request

Open your PR against `main` and fill in the template. A maintainer will review it; CI must
be green before it can be merged. Small, focused PRs are much easier to review and land
quickly.

## Reporting bugs and requesting features

Use the issue templates — they prompt for the input, the exact command, and the expected
vs. actual output, which is what makes a report actionable. Security issues follow a
separate, private process described in [`SECURITY.md`](.github/SECURITY.md).

By contributing, you agree that your contributions are licensed under the same terms as
the project (see [`LICENSE`](LICENSE)).
