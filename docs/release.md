# Releasing to PyPI

The PyPI package `nbdime-rs` ships one wheel per platform. Each wheel holds the `nbdime_rs`
module and the native commands, and is built by `scripts/build_wheel.py`. An sdist that builds
the module only is published alongside. `.github/workflows/release.yml` builds and test-installs
everything on each change to the packaging and on each tag. It publishes only for a `v*` tag,
through PyPI trusted publishing, so no API token is stored anywhere.

## One-time setup (owner)
1. Create a PyPI account at https://pypi.org/account/register/ and turn on two-factor
   authentication.
2. On PyPI, go to Account settings, then Publishing, then "Add a new pending publisher". Choose
   GitHub and enter:
   - PyPI project name: `nbdime-rs`
   - Owner: `Usman-Siddiqi`
   - Repository name: `nbdime-rs`
   - Workflow name: `release.yml`
   - Environment name: `pypi`
3. On GitHub, go to the repository's Settings, then Environments, then New environment, and name
   it `pypi`. Optionally, under "Required reviewers", add yourself, so every publish waits for
   your click.

## Each release
1. Set `version` in the root `Cargo.toml` (workspace package), commit and push to main. Wait for
   the Release workflow to go green on that commit.
2. Tag the commit and push the tag:
   `git tag v0.1.0 && git push origin v0.1.0`
3. The Release workflow builds and tests again, then publishes. It stops if the tag does not
   match the version. If you added required reviewers, approve the `pypi` deployment in the
   workflow run.
4. Check the release with `pip install nbdime-rs`, then `nbdiff --version`.
