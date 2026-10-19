# Contributing to carta

Thanks for your interest in improving carta! This guide covers the mechanics of
contributing.

## Getting set up

The Rust toolchain is pinned in [`rust-toolchain.toml`](rust-toolchain.toml). Use `rustup` to install the right version automatically. A few extra tools are used by the test and lint suites:

```sh
cargo install cargo-nextest      # test runner (required)
cargo install cargo-insta        # snapshot review (Layer 1)
cargo install cargo-llvm-cov     # coverage
cargo install cargo-deny         # dependency/license/advisory checks
cargo install typos-cli          # optional spell check
cargo install hyperfine          # optional, used only by tools/bench-suite
```

Run the one-time developer setup to enable the code quality git hooks:

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
```

### Snapshot tests

Golden output is captured with [`insta`](https://insta.rs). After an intentional change to output, review and accept the new snapshots:

```sh
cargo insta review
```

CI rejects stale or unreferenced snapshots, so keep them tidy.

## Making a change

- One logical change per commit. Commit messages follow
  [Conventional Commits](https://www.conventionalcommits.org/) (`feat`, `fix`, `docs`,
  `refactor`, `perf`, `test`, `build`, `ci`, `chore`, …).
- Keep output deterministic and avoid panics in library paths.
- When you add, extend, or change support for a format or extension, edit
  [`docs/status.toml`](docs/status.toml) and regenerate (see below).

### Generated documentation

[`docs/STATUS.md`](docs/STATUS.md) and the documentation site's format and extension data are
generated from [`docs/status.toml`](docs/status.toml). Edit the TOML file, not the generated files, and re-generate:

```sh
cargo run --manifest-path tools/docgen/Cargo.toml -- --write
```

## Opening a pull request

Open your PR against `main` and fill in the template. A maintainer will review it. Small, focused PRs are much easier to review and land quickly!

## Reporting bugs and requesting features

Use the issue templates. For security issues, please follow the separate, private process described in [`SECURITY.md`](.github/SECURITY.md).
