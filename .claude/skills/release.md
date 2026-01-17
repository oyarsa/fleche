---
name: release
description: Update documentation and cut a release
user-invocable: true
---

Review the changes made in this session and update documentation, then cut a release.

## Step 1: Update Documentation

Update the relevant documentation files:

1. **Guide** (`src/guide.rs`): Add examples and update the Commands Reference table
2. **README** (`README.md`): Update if the change affects the quick start or overview

For each change:
- Add clear, concise examples showing typical usage
- Update any tables or reference sections
- Keep the style consistent with existing documentation

Run `just fix` to verify everything passes, then commit the documentation changes.

## Step 2: Cut Release

Follow the instructions in CLAUDE.md:

1. Bump the version in `Cargo.toml` (minor for features, patch for fixes)
2. Update the release date in `src/cli.rs` (`long_version()` function)
3. Add an entry to `CHANGELOG.md` describing what changed since last release
4. Run `just fix` to verify
5. Commit with message `v<VERSION>: <summary>`
6. Create git tag `v<VERSION>`
7. Push commits and tags
