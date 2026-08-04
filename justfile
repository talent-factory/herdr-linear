# justfile — herdr-linear task runner
#
# Run `just` (or `just --list`) to see all recipes.
# Stack: Rust crate — async Linear GraphQL client (reqwest + tokio).

# Default — show available recipes
default:
    @just --list

# ─── Setup ────────────────────────────────────────────────────────────────────

# Install toolchain updates and dev-only cargo extensions
[group('setup')]
setup:
    @echo "Setting up development environment..."
    rustup update
    cargo install cargo-watch cargo-edit
    @echo "✅ Setup complete!"

# ─── Build ────────────────────────────────────────────────────────────────────

# Build the project in release mode
[group('build')]
build:
    cargo build --release

# Watch for changes and rebuild (+ run lib tests)
[group('build')]
watch:
    cargo watch -x build -x "test --lib"

# Remove build artifacts
[group('build')]
clean:
    cargo clean

# ─── Quality ──────────────────────────────────────────────────────────────────

# Format code with rustfmt
[group('quality')]
fmt:
    cargo fmt --all

# Run clippy linter
[group('quality')]
lint:
    cargo clippy --all-targets --all-features -- -D warnings

# Run all tests
[group('quality')]
test:
    cargo test --all-features -- --nocapture

# Run fmt, lint, and test — the full pre-commit gate
[group('quality')]
check: fmt lint test
    @echo "✅ All checks passed!"

# ─── Run ──────────────────────────────────────────────────────────────────────

# Run the example binary (requires LINEAR_API_KEY)
[group('run')]
run:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ -z "${LINEAR_API_KEY:-}" ]; then
        echo "Error: LINEAR_API_KEY environment variable not set"
        exit 1
    fi
    RUST_LOG=debug cargo run --bin herdr-linear

# ─── Docs ─────────────────────────────────────────────────────────────────────

# Generate and open documentation
[group('docs')]
doc:
    cargo doc --no-deps --open
