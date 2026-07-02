# Releasing carta

Releases are automated with [release-plz](https://release-plz.dev). All five crates share one
version and are released together; every crate is published to [crates.io](https://crates.io) and a
single GitHub release carries prebuilt `carta` binaries.

## How it works

1. You merge ordinary pull requests to `main` using [Conventional Commits](https://www.conventionalcommits.org/).
2. `release-plz` keeps a **Release PR** open that bumps the shared version and regenerates
   `CHANGELOG.md` from those commits (`.github/workflows/release-plz.yml`).
3. You review — and if needed edit — the changelog in that PR, then merge it.
4. Merging publishes every crate to crates.io (via OIDC Trusted Publishing), pushes the `vX.Y.Z`
   tag, and cuts the GitHub release.
5. `.github/workflows/release.yml` then builds `carta` for every target and attaches the archives.

You never bump versions or edit the changelog by hand; the commit messages are the source of truth.

## Versioning

All crates move in lockstep on a single version. While the version is below `0.1.0`, every release
may contain breaking changes — Cargo treats each `0.0.x` as incompatible with the last, which is the
intended signal during alpha.

## One-time setup

Do this once, before the first release. Merging this workflow to `main` activates the release-plz
job on every push; until the GitHub App secrets (step 3) exist, those runs fail harmlessly at the
token step without publishing anything, so the order below can be completed at your own pace.

### 1. Bootstrap the first publish (`0.0.1`)

crates.io has no pending-publisher flow for brand-new crate names, so the very first publish must use
an API token. (Every release after this uses OIDC and needs no token.)

1. Create a crates.io account and generate an API token scoped to **publish-new** and
   **publish-update**.
2. From a clean checkout of `main` at the version-`0.0.1` commit, dry-run then publish all crates in
   dependency order:
   ```sh
   cargo publish --workspace --dry-run
   CARGO_REGISTRY_TOKEN=<token> cargo publish --workspace
   ```
3. Tag the release and create it on GitHub so the binary workflow runs:
   ```sh
   git tag v0.0.1
   git push origin v0.0.1
   gh release create v0.0.1 --title v0.0.1 --notes "Initial alpha release."
   ```

If a publish fails partway through, the crates already on crates.io cannot be re-published at the
same version — fix the problem and cut the next patch version for the remainder.

### 2. Register Trusted Publishing (after `0.0.1` exists)

For **each** of the five crates (`carta`, `carta-ast`, `carta-core`, `carta-readers`,
`carta-writers`), on its crates.io page go to **Settings → Trusted Publishing → Add a
new publisher** and enter:

- Repository owner and name: `mfkrause/carta`
- Workflow filename: `release-plz.yml`
- Environment: _(leave empty)_

This lets the release workflow mint a short-lived publish token via OIDC — no stored registry secret.

### 3. Create the GitHub App for the Release PR

The default `GITHUB_TOKEN` cannot trigger other workflows, so a Release PR opened with it would never
run CI. A GitHub App token avoids that.

1. Create a GitHub App (owner: your account) with **Repository permissions**: _Contents_ →
   Read and write, _Pull requests_ → Read and write.
2. Generate a private key and install the App on the `carta` repository.
3. Add two repository secrets:
   - `RELEASE_PLZ_APP_ID` — the App's numeric ID.
   - `RELEASE_PLZ_APP_PRIVATE_KEY` — the full contents of the generated `.pem` file.

## Cutting a release (steady state)

Nothing manual beyond reviewing and merging:

1. Land the changes you want to release on `main`.
2. Open the **Release PR** that release-plz maintains; check the version bump and changelog, editing
   the changelog in the PR if you want to reword or regroup entries.
3. Merge it. Publishing, tagging, the GitHub release, and the binary uploads all happen
   automatically.
