//! Shared protobuf API surface for the pkdealer workspace.
//!
//! This crate compiles and re-exports the protobuf schema so both binaries use
//! the same generated Rust messages and gRPC service/client stubs.

/// The protobuf package name used by generated gRPC types.
pub const DEALER_PROTO_PACKAGE: &str = "pkdealer.dealer.v1";

/// Binary `FileDescriptorSet` emitted by `tonic-build`, suitable for
/// feeding to `tonic_reflection::server::Builder::register_encoded_file_descriptor_set`.
/// Lets `pkdealer_service` serve gRPC reflection so `grpcurl` and other
/// dynamic clients can introspect the API without a local `.proto` file.
pub const DEALER_FILE_DESCRIPTOR_SET: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/dealer_descriptor.bin"));

/// Generated protobuf messages and gRPC definitions for the dealer API.
pub mod dealer {
    tonic::include_proto!("pkdealer.dealer.v1");
}

/// Creates a basic ping request used for connectivity checks.
///
/// This helper exists so downstream crates can build request values without
/// repeating field names or allocating intermediate structures.
///
/// # Parameters
/// - `client_id`: Identifier for the caller issuing the ping request.
///
/// # Returns
/// A [`dealer::PingRequest`] populated with the provided `client_id`.
///
/// # Examples
/// ```rust
/// use pkdealer_proto::{new_ping_request, DEALER_PROTO_PACKAGE};
///
/// let request = new_ping_request("client-1");
/// assert_eq!(request.client_id, "client-1");
/// assert_eq!(DEALER_PROTO_PACKAGE, "pkdealer.dealer.v1");
/// ```
pub fn new_ping_request(client_id: impl Into<String>) -> dealer::PingRequest {
    dealer::PingRequest {
        client_id: client_id.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_ping_request_happy_path() {
        let request = new_ping_request("client-42");
        assert_eq!(request.client_id, "client-42");
    }

    #[test]
    fn new_ping_request_empty_client_id() {
        let request = new_ping_request("");
        assert!(request.client_id.is_empty());
    }

    #[test]
    #[allow(clippy::expect_used)]
    fn seat_info_round_trips_token_fields() {
        use crate::dealer::SeatInfo;
        use prost::Message;

        let seat = SeatInfo {
            seat_number: 2,
            player_name: "gto".to_owned(),
            input_tokens: 1200,
            output_tokens: 8,
            ..Default::default()
        };

        let bytes = seat.encode_to_vec();
        let decoded = SeatInfo::decode(bytes.as_slice()).expect("decode SeatInfo");

        assert_eq!(decoded.input_tokens, 1200);
        assert_eq!(decoded.output_tokens, 8);
    }
}
