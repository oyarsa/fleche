# Claude Code Instructions

Project-specific instructions for working on fleche.

## Before Completing Any Task

1. Run `just check-all` (runs clippy, rustfmt check, and tests)
2. Bump the version in `Cargo.toml` (see Versioning below)
3. Add an entry to `CHANGELOG.md` describing what changed
4. Commit, create a git tag (e.g., `v3.1.0`), and push both

## Commits

- Do NOT add `Co-Authored-By` lines to commits
- Use natural, descriptive commit messages (no "conventional commits" format)
- Commit messages should explain what and why, not how

## Versioning

Simple incrementing: `MAJOR.MINOR.0` (e.g., `2.0.0`, `2.1.0`, `3.0.0`)

- **Major**: bump for new releases with features (1.0.0 → 2.0.0)
- **Minor**: bump for fixes/patches to a release (2.0.0 → 2.1.0)
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
