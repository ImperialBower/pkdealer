# Quick Reference: pkgrpc Binary Subprojects

## Two Binary Crates

1. **pkdealer_service** - gRPC server (port 50051)
2. **pkdealer_client** - CLI client

## Essential Commands

### Build
```bash
cargo build --workspace              # Build both
cargo build -p pkdealer_service     # Build service only
cargo build -p pkdealer_client      # Build client only
```

### Run
```bash
cargo run --bin pkdealer_service    # Start server
cargo run --bin pkdealer_client     # Run client
```

### Test
```bash
cargo test --workspace              # Test both
cargo test -p pkdealer_service      # Test service
cargo test -p pkdealer_client       # Test client
```

### Quality
```bash
cargo fmt --workspace               # Format code
cargo clippy --workspace            # Lint code
cargo doc --workspace --open        # Generate docs
cargo deny check                    # Check licenses
```

## File Locations

```
crates/
├── pkdealer_service/
│   ├── Cargo.toml      # Service config
│   ├── README.md       # Service docs
│   └── src/main.rs     # Service code
└── pkdealer_client/
    ├── Cargo.toml      # Client config
    ├── README.md       # Client docs
    └── src/main.rs     # Client code
```

## Key Features

✅ Fully documented with doc tests
✅ Unit tests for all functions
✅ Proper error handling (no unwrap)
✅ GPL-3.0-or-later licensed
✅ CI/CD integrated
✅ Ready for gRPC implementation

## Next Steps

1. Add gRPC dependencies (tonic, prost)
2. Define .proto files
3. Implement service logic
4. Implement client logic

See `SUBPROJECTS_ADDED.md` for complete details.

