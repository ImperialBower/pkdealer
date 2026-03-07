# Poker Dealer Service

A gRPC service implementation for poker dealer functionality.

## Overview

This service provides a gRPC interface for poker game operations including:

- **Card Management**: Shuffling and dealing cards
- **Game State**: Managing poker game state and player positions
- **Rule Enforcement**: Validating game actions and enforcing poker rules
- **Session Management**: Handling multiple concurrent game sessions

## Building

```bash
# From workspace root
cargo build --package pkdealer_service

# Or from this directory
cargo build
```

## Running

```bash
# From workspace root
cargo run --bin pkdealer_service

# Or from this directory
cargo run
```

## Testing

```bash
# Run all tests
cargo test --package pkdealer_service

# Run with output
cargo test --package pkdealer_service -- --nocapture

# Run doc tests
cargo test --package pkdealer_service --doc
```

## Configuration

Configuration options will be loaded from:
- Environment variables
- Configuration file (`config.toml`)
- Command-line arguments

### Environment Variables

- `PKDEALER_PORT` - Service port (default: 50051)
- `PKDEALER_HOST` - Bind address (default: 0.0.0.0)
- `PKDEALER_LOG_LEVEL` - Logging level (default: info)

## API

The service exposes the following gRPC methods:

- `ShuffleDeck()` - Create and shuffle a new deck
- `DealCards()` - Deal specified number of cards
- `CreateSession()` - Create a new game session
- `GetGameState()` - Query current game state

See the proto definitions for detailed API documentation.

## Development

### Prerequisites

- Rust 1.85.0 or later
- Protocol Buffers compiler (protoc)

### Code Style

This project follows the guidelines in `.github/copilot-instructions.md`:
- All public functions must have doc tests
- All public functions must have unit tests
- No `unwrap()` or `panic!()` in library code
- Comprehensive error handling

## License

GPL-3.0-or-later

See [LICENSE-GPL3.0](../../LICENSE-GPL3.0) for details.

