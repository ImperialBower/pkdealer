# Poker Dealer Client

A gRPC client for interacting with the poker dealer service.

## Overview

This client provides a command-line interface to interact with the poker dealer service:

- **Game Operations**: Request card deals, shuffle deck, query game state
- **Session Management**: Create and join game sessions
- **Interactive Mode**: REPL for testing and development
- **Batch Mode**: Execute commands from scripts

## Building

```bash
# From workspace root
cargo build --package pkdealer_client

# Or from this directory
cargo build
```

## Running

```bash
# From workspace root
cargo run --bin pkdealer_client

# Or from this directory
cargo run
```

## Usage

### Interactive Mode

```bash
# Start interactive session
cargo run --bin pkdealer_client

# Then use commands:
> connect localhost:50051
> shuffle
> deal 5
> state
> quit
```

### Command-Line Mode

```bash
# Connect and shuffle
cargo run --bin pkdealer_client -- --server localhost:50051 shuffle

# Deal cards
cargo run --bin pkdealer_client -- --server localhost:50051 deal --count 5

# Query game state
cargo run --bin pkdealer_client -- --server localhost:50051 state
```

## Testing

```bash
# Run all tests
cargo test --package pkdealer_client

# Run with output
cargo test --package pkdealer_client -- --nocapture

# Note: Binary crates don't have separate library doc tests
# Doc tests in function comments are checked during regular test runs
```

## Configuration

Configuration options can be specified via:
- Command-line arguments
- Environment variables
- Configuration file (`~/.pkdealer/config.toml`)

### Environment Variables

- `PKDEALER_SERVER` - Service address (default: localhost:50051)
- `PKDEALER_TIMEOUT` - Request timeout in seconds (default: 10)
- `PKDEALER_LOG_LEVEL` - Logging level (default: info)

## Commands

### Connection

- `connect <address>` - Connect to service
- `disconnect` - Disconnect from service
- `status` - Show connection status

### Game Operations

- `shuffle` - Shuffle the deck
- `deal <count>` - Deal specified number of cards
- `state` - Display current game state
- `reset` - Reset game to initial state

### Session Management

- `create-session <name>` - Create new game session
- `join-session <id>` - Join existing session
- `list-sessions` - List active sessions

### Utility

- `help` - Show available commands
- `quit` - Exit client

## Development

### Prerequisites

- Rust 1.85.0 or later
- Protocol Buffers compiler (protoc)
- Running instance of pkdealer_service

### Code Style

This project follows the guidelines in `.github/copilot-instructions.md`:
- All public functions must have doc tests
- All public functions must have unit tests
- No `unwrap()` or `panic!()` in library code
- Comprehensive error handling

## License

GPL-3.0-or-later

See [LICENSE-GPL3.0](../../LICENSE-GPL3.0) for details.

