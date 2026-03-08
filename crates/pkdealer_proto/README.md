# pkdealer_proto

Shared protobuf definitions and generated Rust types for the `pkdealer` workspace.

## Layout

- `proto/`: source `.proto` files
- `build.rs`: protobuf/gRPC code generation
- `src/lib.rs`: public re-exports and small convenience helpers

## Usage

Add this crate as a dependency, then import generated types:

```rust
use pkdealer_proto::dealer::PingRequest;
use pkdealer_proto::new_ping_request;

let _request = PingRequest {
    client_id: "client-1".to_owned(),
};
let _also_request = new_ping_request("client-1");
```

## Development

```bash
cargo test -p pkdealer_proto
cargo test --doc -p pkdealer_proto
```

