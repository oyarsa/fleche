# Agent instructions

Project-specific instructions for working on fleche.

## Before Completing Any Task

1. Run `just fix` (formats code, autofixes clippy warnings, and runs tests)
2. Commit changes (do NOT bump version or tag unless explicitly asked)

## Releasing

Only release when explicitly requested (e.g., "release", "cut a release", "bump version").

When releasing:
1. Bump the version in `Cargo.toml` (see Versioning below)
2. Update the release date in `src/cli.rs` (`long_version()` function)
3. Add an entry to `CHANGELOG.md` describing what changed since last release
4. `jj commit -m "v3.1.0: Release summary"`
5. `jj bookmark set master -r @-`
6. `jj git push`
7. `git tag v3.1.0 && git push --tags` (jj doesn't handle tags yet — pushing the tag triggers the release workflow automatically)

## Version Control

Use **jj** (Jujutsu), not git, for all version control operations:

- `jj status` - check working copy status
- `jj log` - view commit history
- `jj commit -m "message"` - create new commit with message for current change
- `jj squash` - squash current change into parent
- `jj bookmark set master` - move master bookmark to current commit
- `jj git push` - push to remote

Typical workflow:
1. Make changes (jj auto-tracks them)
2. `jj commit -m "Your commit message"` to commit the change
4. `jj bookmark set master` to update master
5. `jj git push` when ready to push

## Commits

- Do NOT add `Co-Authored-By` lines to commits
- Use natural, descriptive commit messages (no "conventional commits" format)
- Commit messages should explain what and why, not how
- Commit summary lines MUST be less than 70 characters

## Versioning

Standard semver: `MAJOR.MINOR.PATCH` (e.g., `5.0.0`, `5.0.1`, `5.1.0`)

- **Major**: breaking changes or major new features (1.0.0 → 2.0.0)
- **Minor**: new features, backwards compatible (5.0.0 → 5.1.0)
- **Patch**: bug fixes (5.0.0 → 5.0.1)
- Update `CHANGELOG.md` when bumping version

## Code Style

Beyond what clippy and rustfmt enforce:

- Favor pure functions over stateful methods
- Prefer immutable data structures
- Think functional programming over object-oriented/procedural
- Extract testable pure logic from impure functions (I/O, network, etc.)
- Avoid `.unwrap()` in production code; use `.expect("reason")` if a panic is truly justified, or propagate errors with `?`

## Testing

- No mocks - if something needs mocking, refactor to extract pure functions instead
- No slow tests - unit tests should be fast
- When a test fails, evaluate whether the test or implementation is wrong before fixing
- Prefer testing via pure functions that take inputs and return outputs
