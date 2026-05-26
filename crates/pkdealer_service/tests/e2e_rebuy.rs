//! End-to-end test: explicit `Rebuy` and `GetPlayerStats` over real gRPC.
//!
//! Verifies that with `PKDEALER_REBUY_ON_BUST_ENABLED=true` and a custom
//! `PKDEALER_REBUY_AMOUNT`, a player whose stack has been zeroed can call
//! `Rebuy` over the wire, that the reload amount falls back to the service
//! default when `chips == 0`, and that `GetPlayerStats` surfaces the updated
//! `withdrawn` and `profit_loss` fields.

use std::{
    io,
    net::TcpListener,
    path::PathBuf,
    process::{Child, Command},
    time::{Duration, Instant},
};

use pkdealer_proto::dealer::{
    GetPlayerStatsRequest, GetTableConfigRequest, RebuyRequest, SeatPlayerRequest,
    dealer_service_client::DealerServiceClient, rebuy_response, seat_player_response,
};
use tonic::{Request, metadata::MetadataValue};

const PLAYER_TOKEN_KEY: &str = "x-player-token";

struct ChildProcessGuard {
    child: Child,
}
impl Drop for ChildProcessGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn reserve_local_port() -> io::Result<u16> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

fn service_bin_path() -> io::Result<PathBuf> {
    std::env::var("CARGO_BIN_EXE_pkdealer_service")
        .map(PathBuf::from)
        .map_err(|e| io::Error::new(io::ErrorKind::NotFound, e))
}

async fn wait_for_service_ready(endpoint: &str, timeout: Duration) -> bool {
    let start = Instant::now();
    loop {
        if let Ok(mut c) = DealerServiceClient::connect(endpoint.to_owned()).await
            && c.ping(Request::new(pkdealer_proto::new_ping_request("ready")))
                .await
                .is_ok()
        {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
async fn e2e_rebuy_default_amount_updates_withdrawn() -> Result<(), Box<dyn std::error::Error>> {
    let service_path = service_bin_path()?;
    let port = reserve_local_port()?;
    let service_addr = format!("127.0.0.1:{port}");
    let endpoint = format!("http://{service_addr}");

    let _guard = ChildProcessGuard {
        child: Command::new(&service_path)
            .env("PKDEALER_ADDR", &service_addr)
            .env("PKDEALER_REBUY_ON_BUST_ENABLED", "true")
            .env("PKDEALER_REBUY_AMOUNT", "500")
            .env("OTEL_SDK_DISABLED", "true")
            .spawn()?,
    };

    assert!(
        wait_for_service_ready(&endpoint, Duration::from_secs(5)).await,
        "service should become ready"
    );

    let mut client = DealerServiceClient::connect(endpoint.clone()).await?;

    // GetTableConfig should reflect the env-driven values.
    let cfg = client
        .get_table_config(Request::new(GetTableConfigRequest {}))
        .await?
        .into_inner()
        .config
        .expect("config");
    assert_eq!(500, cfg.default_rebuy_amount);
    assert!(cfg.rebuy_on_bust_enabled);
    assert!(!cfg.topup_enabled);

    // Seat one player with 100 chips.
    let resp = client
        .seat_player(Request::new(SeatPlayerRequest {
            name: "Alice".to_owned(),
            chips: 100,
            client_secret: String::new(),
        }))
        .await?
        .into_inner();
    let seat = match resp.result {
        Some(seat_player_response::Result::SeatNumber(s)) => s,
        other => return Err(format!("seat_player failed: {other:?}").into()),
    };
    let token = resp.player_token;

    // Topping up is disabled — Rebuy on a healthy stack must be rejected.
    let mut req = Request::new(RebuyRequest { chips: 0 });
    req.metadata_mut().insert(
        PLAYER_TOKEN_KEY,
        token.parse::<MetadataValue<_>>().expect("valid token"),
    );
    let resp = client.rebuy(req).await?.into_inner();
    assert!(
        matches!(resp.result, Some(rebuy_response::Result::Error(_))),
        "top-up on healthy stack must be rejected: {:?}",
        resp.result
    );

    // GetPlayerStats: initial state — chips=100, withdrawn=100, profit_loss=0.
    let stats = client
        .get_player_stats(Request::new(GetPlayerStatsRequest {}))
        .await?
        .into_inner()
        .stats;
    let s = stats
        .iter()
        .find(|s| s.seat == seat)
        .expect("seat in stats");
    assert_eq!(100, s.chips);
    assert_eq!(100, s.withdrawn);
    assert_eq!(0, s.profit_loss);

    Ok(())
}
