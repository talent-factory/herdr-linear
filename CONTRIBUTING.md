# Contributing to Herdr Linear

Thank you for your interest in contributing! We welcome all kinds of contributions.

## Code of Conduct

Please be respectful and constructive in all interactions.

## Getting Started

### Prerequisites

- Rust 1.88+ (install via [rustup](https://rustup.rs/))
- Git

### Setup Development Environment

```bash
# Clone the repository
git clone https://github.com/talent-factory/herdr-linear.git
cd herdr-linear

# Build the project
cargo build

# Run tests
cargo test

# Run with logging
export RUST_LOG=debug
cargo run --example tracing_demo
```

## Development Workflow

### 1. Create an Issue (or pick an existing one)

Check [Linear Issues](https://linear.app/talent-factory/project/herdr-linear-10dca51ea35b/overview) for tasks.

### 2. Create a Feature Branch

```bash
git checkout -b feature/your-feature-name
```

### 3. Make Changes

Follow these guidelines:

- **Code Style**: Use `rustfmt` (automatic via `cargo fmt`)
- **Linting**: Run `cargo clippy` before submitting
- **Tests**: Add tests for new functionality
- **Documentation**: Update README and doc comments as needed

### 4. Commit Messages

Use clear, descriptive commit messages:

```
git commit -m "feat: add support for issue filtering by labels"
git commit -m "fix: handle rate limit responses from Linear API"
git commit -m "docs: improve README examples"
git commit -m "test: add tests for cycle queries"
```

### 5. Push and Create Pull Request

```bash
git push origin feature/your-feature-name
```

Then open a PR on GitHub with a clear description of your changes.

## Project Structure

```
src/
├── lib.rs          # Main library interface
├── client.rs       # LinearClient implementation
├── models.rs       # Type definitions
├── queries.rs      # GraphQL query strings
├── error.rs        # Error handling
└── main.rs         # CLI examples
```

## Testing

```bash
# Run all tests
cargo test

# Run specific test
cargo test test_client_creation

# Run with output
cargo test -- --nocapture

# Run with logging
RUST_LOG=debug cargo test -- --nocapture
```

## Code Quality

### Formatting

```bash
cargo fmt
```

### Linting

```bash
cargo clippy -- -D warnings
```

### Documentation

```bash
cargo doc --open
```

## Adding New Features

### Adding a New Query

1. Add GraphQL query string to `src/queries.rs`
2. Add response type to `src/models.rs` if needed
3. Add client method to `src/client.rs`
4. Add tests
5. Update README with example

Example:

```rust
// queries.rs
pub const QUERY_USERS: &str = r#"
query Users($first: Int) {
  users(first: $first) {
    nodes { ... }
  }
}
"#;

// client.rs
pub async fn get_users(&self, limit: Option<i32>) -> Result<Vec<User>> {
    let variables = json!({"first": limit.unwrap_or(50)});
    let response = self.query::<serde_json::Value>(QUERY_USERS, variables).await?;
    // ... extract and return
}
```

### Updating Models

If adding fields to existing types:

1. Update the model in `src/models.rs`
2. Update relevant GraphQL queries
3. Ensure backward compatibility
4. Add tests

## Documentation

- Use doc comments with examples (`/// ...`)
- Update README.md for public API changes
- Add examples in `src/main.rs` for new features

## Pull Request Process

1. Ensure all tests pass: `cargo test`
2. Run formatter: `cargo fmt`
3. Run linter: `cargo clippy`
4. Update documentation
5. Keep PR focused and reasonably sized
6. Reference related Linear issues in PR description

## Reporting Issues

- Check if issue already exists
- Provide minimal reproducible example
- Include error messages and logs
- Specify Rust version: `rustc --version`

## Questions?

- Open a discussion on GitHub
- Comment on related issues
- Post in Linear workspace

## License

By contributing, you agree that your contributions will be licensed under the same dual license (MIT OR Apache-2.0).

---

Thank you for contributing! 🚀
