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
fix:
    cargo fmt
    cargo clippy --all-targets --fix --allow-dirty --allow-staged -- -D warnings
    cargo test --quiet
