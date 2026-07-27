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
cargo install typos-cli          # optional spell check, for CI parity via tools/check.sh
cargo install hyperfine          # optional, used only by tools/bench-suite
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

### Snapshot tests

Golden output is captured with [`insta`](https://insta.rs). After an intentional change
to output, review and accept the new snapshots:

```sh
cargo insta review
```

CI rejects stale or unreferenced snapshots, so keep them tidy.

## Making a change

- Branch off `main`; do not commit directly to `main`.
- One logical change per commit. Commit messages follow
  [Conventional Commits](https://www.conventionalcommits.org/) (`feat`, `fix`, `docs`,
  `refactor`, `perf`, `test`, `build`, `ci`, `chore`, …); the `commit-msg` hook enforces
  the format.
- Keep output deterministic and avoid panics in library paths.
- When you add, extend, or change support for a format or extension, edit
  [`docs/status.toml`](docs/status.toml) and regenerate (see below).

### Generated documentation

[`docs/STATUS.md`](docs/STATUS.md) and the documentation site's format and extension data are
generated from [`docs/status.toml`](docs/status.toml). Edit the TOML, never the generated files,
then regenerate:

```sh
cargo run --manifest-path tools/docgen/Cargo.toml -- --write
```

CI runs the same generator in `--check` mode and fails if a committed artifact has drifted. A test
also cross-checks the format registry against `docs/status.toml`, so adding a format without
documenting it fails the suite.

[`docs/BENCHMARKS.md`](docs/BENCHMARKS.md) and the site's chart data come from the same generator,
reading [`docs/benchmarks.toml`](docs/benchmarks.toml). That file holds measurements only, so it is
refreshed by re-running the suite rather than by editing:

```sh
tools/bench-suite/run.sh all --json /tmp/bench.json
tools/bench-suite/to-toml.sh /tmp/bench.json >docs/benchmarks.toml
cargo run --manifest-path tools/docgen/Cargo.toml -- --write
```

Benchmarks never gate a PR: the `Benchmarks` workflow runs on demand and after a release, and opens
a PR with the refreshed numbers.

## Opening a pull request

Open your PR against `main` and fill in the template. A maintainer will review it; CI must
be green before it can be merged. Small, focused PRs are much easier to review and land
quickly.

## Reporting bugs and requesting features

Use the issue templates. They prompt for the input, the exact command, and the expected
vs. actual output, which is what makes a report actionable. Security issues follow a
separate, private process described in [`SECURITY.md`](.github/SECURITY.md).
