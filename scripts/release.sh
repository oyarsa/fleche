#!/usr/bin/env bash
# Automated release script for fleche.
#
# Usage:
#   scripts/release.sh [major|minor|patch]
#
# Defaults to "minor" if not specified.
# Runs claude -p to generate documentation updates and changelog,
# then performs the mechanical release steps.

set -euo pipefail

BUMP_TYPE="${1:-minor}"

if [[ "$BUMP_TYPE" != "major" && "$BUMP_TYPE" != "minor" && "$BUMP_TYPE" != "patch" ]]; then
    echo "Usage: $0 [major|minor|patch]"
    exit 1
fi

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

current_version() {
    grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/'
}

next_version() {
    local cur="$1" bump="$2"
    local major minor patch
    IFS='.' read -r major minor patch <<< "$cur"
    if [[ "$bump" == "major" ]]; then
        echo "$((major + 1)).0.0"
    elif [[ "$bump" == "minor" ]]; then
        echo "${major}.$((minor + 1)).0"
    else
        echo "${major}.${minor}.$((patch + 1))"
    fi
}

# ---------------------------------------------------------------------------
# Pre-flight checks
# ---------------------------------------------------------------------------

if ! command -v claude &>/dev/null; then
    echo "Error: claude CLI not found"
    exit 1
fi

if ! command -v jj &>/dev/null; then
    echo "Error: jj (Jujutsu) not found"
    exit 1
fi

# Ensure we're in the project root
if [[ ! -f Cargo.toml ]] || [[ ! -f CLAUDE.md ]]; then
    echo "Error: run this script from the fleche project root"
    exit 1
fi

# Check for uncommitted work (jj always auto-tracks, so check for changes)
if jj diff --stat 2>/dev/null | grep -qv '^0 files changed'; then
    echo "Error: working copy has uncommitted changes — commit or shelve first"
    exit 1
fi

OLD_VERSION=$(current_version)
NEW_VERSION=$(next_version "$OLD_VERSION" "$BUMP_TYPE")
TODAY=$(date +%Y-%m-%d)

echo "Releasing: v${OLD_VERSION} → v${NEW_VERSION} (${BUMP_TYPE})"
echo ""

# ---------------------------------------------------------------------------
# Step 1: Gather commit log since last tag
# ---------------------------------------------------------------------------

LAST_TAG=$(git describe --tags --abbrev=0 2>/dev/null || echo "")
if [[ -z "$LAST_TAG" ]]; then
    echo "Error: no previous git tag found"
    exit 1
fi

echo "Changes since ${LAST_TAG}:"
COMMIT_LOG=$(jj log -r "${LAST_TAG}..@-" --no-graph 2>/dev/null || git log "${LAST_TAG}..HEAD" --oneline)
echo "$COMMIT_LOG"
echo ""

DIFF_STAT=$(jj diff -r "${LAST_TAG}..@-" --stat 2>/dev/null || git diff "${LAST_TAG}..HEAD" --stat)

# ---------------------------------------------------------------------------
# Step 2: Claude generates documentation updates + changelog
# ---------------------------------------------------------------------------

echo "Running claude to update docs and changelog..."
echo ""

PROMPT="You are releasing fleche v${NEW_VERSION} (${TODAY}).

Previous version: ${OLD_VERSION} (tag: ${LAST_TAG})
Bump type: ${BUMP_TYPE}

Commits since last release:
${COMMIT_LOG}

Diff stats:
${DIFF_STAT}

Do the following:

1. Read the current src/guide.rs, README.md, and CHANGELOG.md
2. Update src/guide.rs if any new commands, flags, or workflows were added
3. Update README.md only if the changes affect the quick start or feature overview
4. Add a new entry to CHANGELOG.md for [${NEW_VERSION}] - ${TODAY}, following the existing style
5. Bump the version in Cargo.toml from \"${OLD_VERSION}\" to \"${NEW_VERSION}\"
6. Update the release date in src/cli.rs (the long_version() function) to \"${TODAY}\"
7. Run \`just fix\` to verify everything passes

Do NOT commit, tag, or push — just make the file edits and run just fix."

claude -p \
    --allowedTools "Read,Edit,Bash(just fix),Bash(cargo *),Glob,Grep" \
    --permission-mode bypassPermissions \
    --max-budget-usd 1 \
    "$PROMPT"

# ---------------------------------------------------------------------------
# Step 3: Verify just fix passes
# ---------------------------------------------------------------------------

echo ""
echo "Verifying build..."
just fix

# ---------------------------------------------------------------------------
# Step 4: Show what changed and ask for confirmation
# ---------------------------------------------------------------------------

echo ""
echo "=== Changes to be released ==="
jj diff --stat
echo ""
jj diff

echo ""
read -rp "Proceed with v${NEW_VERSION} release? [y/N] " confirm
if [[ "$confirm" != "y" && "$confirm" != "Y" ]]; then
    echo "Aborted. Changes are still in the working copy — review and commit manually."
    exit 1
fi

# ---------------------------------------------------------------------------
# Step 5: Commit, bookmark, push, tag
# ---------------------------------------------------------------------------

# Build a short summary from the first bullet point under the version heading
SUMMARY=$(sed -n "/^## \[${NEW_VERSION}\]/,/^## \[/{ /^- /{ s/^- //; p; q; }; }" CHANGELOG.md | head -c 50)
COMMIT_MSG="v${NEW_VERSION}: ${SUMMARY}"

# Enforce 70-char limit
if [[ ${#COMMIT_MSG} -gt 70 ]]; then
    COMMIT_MSG="${COMMIT_MSG:0:67}..."
fi

jj commit -m "$COMMIT_MSG"
jj bookmark set master -r @-
jj git push

git tag "v${NEW_VERSION}"
git push --tags

echo ""
echo "Released v${NEW_VERSION}"
