# Claude Instructions

These instructions guide Claude to generate code that aligns with our project standards for testing, documentation, and code quality.

## Testing Requirements

### Unit Tests
- **Every public function must have at least one unit test** covering the happy path
- **Every public struct/enum must have tests** that validate construction, methods, and trait implementations
- Unit tests should be placed in a `#[cfg(test)]` module at the end of the file or in a `tests/` directory
- Test names should be descriptive and follow the pattern: `<function_or_struct_name>_<scenario>`. **Do not prefix test function names with `test_`** — the `#[test]` / `#[tokio::test]` attribute already marks them as tests, so the prefix is redundant noise (e.g. `fn parses_hole_cards`, not `fn test_parses_hole_cards`).
- Include edge cases, error conditions, and boundary conditions
- Use `assert!`, `assert_eq!`, and `assert_ne!` macros effectively

### Doc Tests
- **Every public function and method must include at least one doc test** demonstrating basic usage
- Doc tests should be included in the doc comment (`///`) using triple backticks with `rust` syntax highlighting
- Doc tests must compile and run successfully
- Doc tests should show the most common usage pattern
- If a function can fail, include a doc test that shows success cases
- Doc tests should be runnable with `cargo test --doc`

### Example Structure
```rust
/// Brief description of what this function does.
///
/// Longer explanation of behavior, parameters, and return values.
///
/// # Panics
/// Panics if the condition X is violated.
///
/// # Errors
/// Returns `Err` if the operation fails due to reason Y.
///
/// # Examples
/// ```
/// use pkcore::your_module::your_function;
/// let result = your_function(42);
/// assert_eq!(result, Ok(expected_value));
/// ```
pub fn your_function(param: Type) -> Result<ReturnType, ErrorType> {
    // implementation
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn your_function_happy_path() {
        let result = your_function(42);
        assert!(result.is_ok());
    }

    #[test]
    fn your_function_edge_case() {
        let result = your_function(0);
        assert!(result.is_err());
    }

    #[test]
    fn your_function_boundary() {
        // Test boundary conditions
        let result = your_function(u32::MAX);
        assert!(result.is_ok());
    }
}
```

## Documentation Requirements

### For Functions
- **Brief summary**: First line should be a concise, single-sentence description
- **Detailed explanation**: Explain what the function does, why it exists, when to use it, and any important caveats
- **Parameters**: Document all parameters with their types, ranges, and expected behavior
- **Return value**: Clearly explain what is returned and under what conditions
- **Errors**: Document all possible error cases using `# Errors` section with explanation
- **Panics**: Document if the function can panic using `# Panics` section
- **Examples**: Include `# Examples` with working doc test code that demonstrates usage
- **Safety**: Use `# Safety` section if the function is `unsafe`, explaining why it's unsafe and how to use safely

### For Structs and Enums
- **Brief description**: Explain the purpose and role of the type in the system
- **Fields**: Document each field with its purpose and expected invariants
- **Examples**: Show common construction and usage patterns
- **Trait implementations**: Explain behavior of implemented traits if non-obvious
- Include examples showing how to construct and interact with the type

### For Modules
- **Module-level documentation**: Each module file should start with a module-level doc comment explaining its purpose
- Example: `//! Handles poker hand ranking and comparison logic.`
- Include examples of common usage patterns for the module

### For Crate Root (lib.rs)
- **Comprehensive overview**: Explain the crate's purpose and main concepts
- **Module organization**: Link to key modules and their purposes
- **Quick start guide**: Show how to get started with the crate
- **Examples**: Provide complete working examples of common tasks

## Code Quality Standards

### Naming Conventions
- Use clear, descriptive names for functions, variables, and types
- Use `snake_case` for functions, variables, and module names
- Use `PascalCase` for types, structs, enums, and traits
- Avoid single-letter variable names except for loop indices (i, j, k)
- Use full words in variable names (e.g., `cards` instead of `c`, `rank` instead of `r`)

### Error Handling

**`unwrap()` / `expect()` / `panic!()` are forbidden in non-test code, and fine in test code.**

- **Forbidden** in everything that ships: library code, binary `main`/CLI paths,
  and any helper reachable from them. Return `Result<T, E>` (or `Option<T>`) and
  propagate with `?`. For a value you can prove is present, prefer a total
  construct — `unwrap_or`, `unwrap_or_default`, `unwrap_or_else`, `map_or`,
  `let ... else`, or a `match` — over `unwrap()` with a comment.
- **Allowed** in `#[cfg(test)]` modules, `tests/` integration tests, test-only
  fixture/helper modules, benches, and **doc-test examples** (a `///` example
  that ends in `.unwrap()` is idiomatic and reads better than error plumbing).
  A panic in a test *is* the failure report.
- **Encode it, don't just document it.** Crate roots carry
  `#![warn(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]`, which
  also fires inside `#[cfg(test)]` modules and makes
  `cargo clippy --all-targets` noisy with unactionable test warnings. Pair the
  warn with a test-scoped allow at the crate root so the lint means what this
  rule says:

  ```rust
  #![warn(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]
  // unwrap/expect are the idiomatic failure report in tests; the ban above is
  // for shipping code only.
  #![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
  ```

  Test-only modules that are themselves `#[cfg(test)]` (e.g. `fixtures.rs`) may
  instead carry a module-level `#![allow(clippy::unwrap_used)]`.
- The enforced gate is lib+bin (`cargo clippy -p <crate> -- -D warnings`), which
  does not compile `cfg(test)` code — so a clean gate is not evidence that test
  code is lint-free. Use `--all-targets` when you want the whole picture.
- Prefer `Result<T, E>` over `Option<T>` for operations that can fail with meaningful errors
- Create custom error types for domain-specific errors
- Implement `std::error::Error` for custom error types
- Document all error cases clearly
- Use `?` operator for error propagation in library code

### Trait Implementations
- Implement `Display` for user-facing types (those that will be shown to users)
- Implement `Debug` for all public types
- Implement `Default` for types that have a sensible default value
- Implement `Clone` and `Copy` when semantically appropriate
- Document non-obvious trait behavior in the trait implementation

### Code Organization
- Keep functions focused and single-purpose
- Extract complex logic into well-named helper functions
- Group related functions and types in logical modules
- Use visibility modifiers (`pub`, `pub(crate)`, private) appropriately

## Rust-Specific Guidelines

### Working with References
- Prefer `&T` (borrowing) over `T` (taking ownership) when possible
- Use `&mut T` only when mutation is needed
- Document borrowing requirements in function documentation
- Consider lifetime requirements for complex references

### Performance Considerations
- Avoid unnecessary cloning; use references when possible
- Use `&[T]` for slices instead of `&Vec<T>`
- Pre-allocate collections with `with_capacity()` when size is known
- Document performance characteristics for expensive operations

### Type Safety
- Use type-safe abstractions instead of primitive types when semantically meaningful
- Use enums instead of strings for fixed sets of values
- Leverage Rust's type system to prevent invalid states at compile time

## Checklist for Code Generation

Before accepting or suggesting code:
- ✓ Does the function have comprehensive doc comments with `# Examples`?
- ✓ Are there unit tests covering happy path, edge cases, and error conditions?
- ✓ Are there doc tests in the doc comments that compile and run?
- ✓ Are all error cases handled and documented?
- ✓ Does **non-test** code avoid `unwrap()`, `expect()`, and `panic!()`? (Tests and doc-test examples may use them freely.)
- ✓ Are edge cases and boundary conditions covered in tests?
- ✓ Is the code readable and maintainable?
- ✓ Are all public APIs documented with examples?
- ✓ Do tests run successfully with `cargo test`?
- ✓ Do doc tests pass with `cargo test --doc`?
- ✓ Does the code follow naming conventions?
- ✓ Are trait implementations documented if behavior is non-obvious?

## Observability

The service is OpenTelemetry-instrumented. Stack config lives in `ops/`;
`docker compose up -d --build` brings up service + collector + Jaeger +
Prometheus + Grafana. See `crates/pkdealer_service/README.md` for env
vars and the full quickstart. Toggle OTel off with `OTEL_SDK_DISABLED=true`
when running tests or `cargo run` without a collector.

## Knowledge Bundle (OKF)

This repo keeps an [Open Knowledge Format](https://github.com/GoogleCloudPlatform/knowledge-catalog/blob/main/okf/SPEC.md)
bundle at `.okf/` — markdown concepts with YAML frontmatter describing the
workspace crates, the DealerService gRPC contract, runbooks (arena,
observability, developer workflow), and the EPIC doc convention.

- **Consume**: read `.okf/index.md` first for orientation, then follow links
  into only the concepts relevant to the task.
- **Maintain**: when a change touches something the bundle describes (new
  crate, new RPC, changed workflow), update the affected concept and its
  `timestamp`, refresh the relevant `index.md`, and add a dated entry to
  `.okf/log.md`.
- **Validate** before committing bundle changes: `/okf:validate .okf --strict`
  (every non-reserved `.md` needs frontmatter with a non-empty `type`;
  `index.md` files carry no frontmatter, except the root index's
  `okf_version`).

