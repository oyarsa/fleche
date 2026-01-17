---
name: update-docs
description: Update docs/help/guide with latest changes
---

Review the changes made in this session and update the relevant documentation:

1. **CLI help text** (`src/cli.rs`): Update argument descriptions and doc comments
2. **Guide** (`src/guide.rs`): Add examples and update the Commands Reference table
3. **README** (`README.md`): Update if the change affects the quick start or overview

For each change:
- Add clear, concise examples showing typical usage
- Update any tables or reference sections
- Keep the style consistent with existing documentation

After updating documentation:
1. Run `just fix` to verify everything passes
2. Commit the documentation changes
3. Cut a release following the instructions in CLAUDE.md:
   - Bump version in `Cargo.toml` (minor for features, patch for fixes)
   - Update the release date in `src/cli.rs` (`long_version()` function) if needed
   - Add an entry to `CHANGELOG.md` describing what changed
   - Commit with message `v<VERSION>: <summary>`
   - Create git tag `v<VERSION>`
   - Push commits and tags
