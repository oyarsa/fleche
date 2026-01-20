_default:
    @just --list

# Build the project
build:
    cargo build

# Run the project
run:
    cargo run

# Build with optimizations
release:
    cargo build --release

# Install binary to user path
install:
    cargo install --path .

# Run tests
test:
    cargo test --quiet

# Run clippy linter
clippy:
    cargo clippy --all-targets -- -D warnings

# Run clippy and fix possible errors
clippy-fix:
    cargo clippy --all-targets --fix --allow-dirty --allow-staged 2>/dev/null

# Run rustfmt checker
fmt-check:
    cargo fmt -- --check

# Format code
fmt:
    cargo fmt

# Run all lints
lint: clippy fmt-check

# Check all: lint and test
check-all: lint test

# Fix formatting and clippy warnings, then run tests
fix: fmt clippy-fix clippy test
