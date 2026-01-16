# Claude Code Instructions

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
4. Commit, create a git tag (e.g., `v3.1.0`), and push both

## Commits

- Do NOT add `Co-Authored-By` lines to commits
- Use natural, descriptive commit messages (no "conventional commits" format)
- Commit messages should explain what and why, not how

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

## Testing

- No mocks - if something needs mocking, refactor to extract pure functions instead
- No slow tests - unit tests should be fast
- When a test fails, evaluate whether the test or implementation is wrong before fixing
- Prefer testing via pure functions that take inputs and return outputs
