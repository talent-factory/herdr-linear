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

# ─── Herdr plugin ─────────────────────────────────────────────────────────────

# Uninstall the herdr-linear plugin (if present), rebuild, then link this checkout.
# NOTE: `herdr plugin link` only registers the manifest path — it does NOT run the
# manifest's [[build]] step. The release binary must be rebuilt explicitly, or herdr
# will keep exec'ing whatever stale ./target/release/herdr-linear already exists.
[group('plugin')]
plugin-reinstall:
    #!/usr/bin/env bash
    set -euo pipefail
    existing=$(herdr plugin list --json | jq -r '.result.plugins[] | select(.plugin_id == "herdr-linear") | .source.kind')
    if [ "$existing" = "local" ]; then
        echo "Unlinking existing local herdr-linear plugin..."
        herdr plugin unlink herdr-linear
    elif [ -n "$existing" ]; then
        echo "Uninstalling existing herdr-linear plugin ($existing)..."
        herdr plugin uninstall herdr-linear
    else
        echo "No existing herdr-linear plugin found."
    fi
    echo "Rebuilding release binary (cargo build --release --features plugin)..."
    cargo build --release --features plugin
    echo "Linking herdr-linear plugin from $(pwd)..."
    herdr plugin link .
    echo "✅ Plugin reinstalled"

# ─── Docs ─────────────────────────────────────────────────────────────────────

# Generate and open documentation
[group('docs')]
doc:
    cargo doc --no-deps --open
