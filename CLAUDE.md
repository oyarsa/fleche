# Claude Code Instructions

Project-specific instructions for working on fleche.

## Before Completing Any Task

Always run `just check-all` before saying you're done. This runs clippy, rustfmt check, and tests.

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
