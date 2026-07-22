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

### Snapshot tests

Golden output is captured with [`insta`](https://insta.rs). After an intentional change
to output, review and accept the new snapshots:

```sh
cargo insta review
```

CI rejects stale or unreferenced snapshots, so keep them tidy.

## Making a change

- **Branch off `main`**; do not commit directly to `main`.
- **One logical change per commit.** Commit messages follow
  [Conventional Commits](https://www.conventionalcommits.org/) (`feat`, `fix`, `docs`,
  `refactor`, `perf`, `test`, `build`, `ci`, `chore`, …); the `commit-msg` hook enforces
  the format.
- **Keep output deterministic** and **avoid panics in library paths**.
- When you add, extend, or change support for a format or extension, remember to update [`docs/STATUS.md`](docs/STATUS.md).

## Opening a pull request

Open your PR against `main` and fill in the template. A maintainer will review it; CI must
be green before it can be merged. Small, focused PRs are much easier to review and land
quickly.

## Reporting bugs and requesting features

Use the issue templates — they prompt for the input, the exact command, and the expected
vs. actual output, which is what makes a report actionable. Security issues follow a
separate, private process described in [`SECURITY.md`](.github/SECURITY.md).

## Contributor License Agreement

Before your first pull request can be merged, you need to sign the
[Contributor License Agreement](CLA.md). It's a one-time step: a bot will
prompt you on your first PR, and you sign by posting a single comment. You keep
full ownership of your contribution.

**Why a CLA?** Being upfront about this: carta is licensed under the AGPL-3.0,
and its maintainer also uses it in their own commercial software and may license it commercially to companies that can't use
AGPL software. The CLA grants the maintainer the right to distribute your
contribution under such commercial terms. This model is
what funds carta's development. The public version is available under the
AGPL. If you're not comfortable with this, we absolutely understand! Issues,
discussions, and bug reports are just as valuable and need no signature.

If you contribute on behalf of your employer, your employer may also need to
execute the Corporate CLA (Part B of [`CLA.md`](CLA.md)); see the instructions
at the end of that part.
