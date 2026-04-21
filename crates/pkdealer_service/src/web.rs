#![warn(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]

//! HTTP spectator interface for the `PKDealer` service.
//!
//! Provides two routes:
//! - `GET /` — embedded HTML page driven by the browser `EventSource` API
//! - `GET /events` — Server-Sent Events stream; each event is a JSON [`SpectatorEvent`]
//!
//! All data is hole-card-free. The broadcast channel carries events already built
//! with `CardVisibility::Hidden`, so spectators never receive private information.

use std::{convert::Infallible, sync::Arc};

use axum::{
    Router,
    extract::State,
    response::{
        Html,
        sse::{Event, KeepAlive, Sse},
    },
    routing::get,
};
use pkdealer_proto::dealer::{
    EventType, PlayerState as ProtoPlayerState, Street, TableEvent, TableStatus,
};
use serde::Serialize;
use tokio::{
    net::TcpListener,
    sync::{broadcast, mpsc},
};
use tokio_stream::wrappers::ReceiverStream;

// ── HTML page ─────────────────────────────────────────────────────────────────

/// Embedded spectator HTML page.
///
/// Uses only safe DOM APIs (`textContent`, `createElement`, `createTextNode`)
/// — no `innerHTML` with server-provided strings.
const SPECTATOR_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>PKDealer Spectator</title>
  <style>
    body { font-family: monospace; background: #1a1a1a; color: #e0e0e0; padding: 20px; max-width: 900px; margin: 0 auto; }
    h1   { color: #f0c040; border-bottom: 1px solid #333; padding-bottom: 8px; }
    h3   { color: #aaa; margin-top: 20px; }
    .seat { display: inline-block; background: #2a2a2a; border: 1px solid #444;
            padding: 8px 12px; margin: 4px; border-radius: 6px; min-width: 150px;
            vertical-align: top; }
    .seat.yet_to_act { border-color: #f0c040; }
    .seat.blind      { border-color: #ffa060; }
    .seat.folded     { opacity: 0.4; }
    .seat.out        { opacity: 0.2; }
    .board  { font-size: 1.5em; color: #80c0ff; margin: 10px 0; letter-spacing: 4px; }
    .pot    { color: #80ff80; font-size: 1.1em; }
    .street { color: #ffa060; text-transform: uppercase; font-weight: bold; letter-spacing: 2px; }
    .log    { background: #111; border: 1px solid #333; padding: 10px;
              max-height: 200px; overflow-y: auto; font-size: .85em; }
    .entry  { border-bottom: 1px solid #1e1e1e; padding: 3px 0; }
    #conn   { color: #666; font-size: .85em; margin-bottom: 12px; }
    .label  { color: #888; font-size: .8em; }
  </style>
</head>
<body>
  <h1>PKDealer — Spectator View</h1>
  <div id="conn">Connecting to event stream...</div>

  <div class="label">STREET</div>
  <div class="street" id="street">—</div>

  <div class="label">BOARD</div>
  <div class="board" id="board">—</div>

  <div class="pot" id="pot"></div>

  <h3>Seats</h3>
  <div id="seats"></div>

  <h3>Event Log</h3>
  <div class="log" id="log"></div>

  <script>
    var es = new EventSource('/events');

    es.onopen = function() {
      document.getElementById('conn').textContent = 'Connected';
    };

    es.onerror = function() {
      document.getElementById('conn').textContent = 'Connection lost — reconnecting...';
    };

    es.onmessage = function(e) {
      var ev = JSON.parse(e.data);

      // ── Event log ──────────────────────────────────────────────────────────
      var log   = document.getElementById('log');
      var entry = document.createElement('div');
      entry.className   = 'entry';
      entry.textContent = ev.description;
      log.insertBefore(entry, log.firstChild);
      while (log.children.length > 30) { log.removeChild(log.lastChild); }

      // ── Table status ───────────────────────────────────────────────────────
      if (!ev.status) { return; }
      var s = ev.status;

      document.getElementById('street').textContent = s.current_street || '—';
      document.getElementById('board').textContent  = s.board  || '—';
      document.getElementById('pot').textContent    = s.pot > 0 ? 'Pot: ' + s.pot : '';

      var seatsDiv = document.getElementById('seats');
      while (seatsDiv.firstChild) { seatsDiv.removeChild(seatsDiv.firstChild); }

      s.seats.forEach(function(seat) {
        var d = document.createElement('div');
        d.className = 'seat ' + seat.state;

        var nameEl = document.createElement('strong');
        nameEl.textContent = seat.player_name;
        d.appendChild(nameEl);

        var br1 = document.createElement('br');
        d.appendChild(br1);

        var infoText = document.createTextNode(
          'Seat ' + seat.seat_number + ' \u00b7 ' + seat.chips + ' chips'
        );
        d.appendChild(infoText);

        var br2 = document.createElement('br');
        d.appendChild(br2);

        var stateEl = document.createElement('em');
        stateEl.textContent = seat.state;
        d.appendChild(stateEl);

        seatsDiv.appendChild(d);
      });
    };
  </script>
</body>
</html>"#;

// ── Shared state ──────────────────────────────────────────────────────────────

/// Shared state for axum route handlers.
///
/// Holds only the broadcast sender; handlers subscribe on each new connection.
pub(crate) struct WebState {
    pub event_tx: broadcast::Sender<TableEvent>,
}

// ── Browser-facing types ──────────────────────────────────────────────────────

/// A [`TableEvent`] translated to a browser-safe JSON shape.
///
/// Hole cards are always redacted — spectators never receive private card data.
#[derive(Serialize)]
pub struct SpectatorEvent {
    /// Human-readable event type string (e.g. `"hand_started"`).
    pub event_type: String,
    /// Human-readable description of what happened.
    pub description: String,
    /// Unix millisecond timestamp from the service.
    pub timestamp: u64,
    /// Current table state after the event, or `None` if not yet available.
    pub status: Option<SpectatorSnapshot>,
}

/// A hole-card-free snapshot of the table for the spectator page.
#[derive(Serialize, Clone)]
pub struct SpectatorSnapshot {
    pub seats: Vec<SpectatorSeat>,
    pub board: String,
    pub pot: u32,
    pub current_street: String,
    pub hand_in_progress: bool,
}

/// One seat's public information (no hole cards).
#[derive(Serialize, Clone)]
pub struct SpectatorSeat {
    pub seat_number: u32,
    pub player_name: String,
    pub chips: u32,
    pub state: String,
}

// ── Mapping helpers ───────────────────────────────────────────────────────────

fn event_type_to_str(raw: i32) -> &'static str {
    match EventType::try_from(raw).unwrap_or(EventType::Unspecified) {
        EventType::Unspecified => "unspecified",
        EventType::PlayerSeated => "player_seated",
        EventType::PlayerRemoved => "player_removed",
        EventType::HandStarted => "hand_started",
        EventType::PlayerAction => "player_action",
        EventType::StreetAdvanced => "street_advanced",
        EventType::HandEnded => "hand_ended",
    }
}

fn street_to_str(raw: i32) -> &'static str {
    match Street::try_from(raw).unwrap_or(Street::Unspecified) {
        Street::Unspecified => "unspecified",
        Street::Preflop => "preflop",
        Street::Flop => "flop",
        Street::Turn => "turn",
        Street::River => "river",
    }
}

fn player_state_to_str(raw: i32) -> &'static str {
    match ProtoPlayerState::try_from(raw).unwrap_or(ProtoPlayerState::Unspecified) {
        ProtoPlayerState::Unspecified => "unspecified",
        ProtoPlayerState::Ready => "ready",
        ProtoPlayerState::YetToAct => "yet_to_act",
        ProtoPlayerState::Checked => "checked",
        ProtoPlayerState::Called => "called",
        ProtoPlayerState::Bet => "bet",
        ProtoPlayerState::Raised => "raised",
        ProtoPlayerState::AllIn => "all_in",
        ProtoPlayerState::Folded => "folded",
        ProtoPlayerState::Out => "out",
        ProtoPlayerState::Blind => "blind",
    }
}

fn table_status_to_snapshot(status: &TableStatus) -> SpectatorSnapshot {
    SpectatorSnapshot {
        seats: status
            .seats
            .iter()
            .map(|s| SpectatorSeat {
                seat_number: s.seat_number,
                player_name: s.player_name.clone(),
                chips: s.chips,
                state: player_state_to_str(s.state).to_owned(),
            })
            .collect(),
        board: status.board.clone(),
        pot: status.pot,
        current_street: street_to_str(status.current_street).to_owned(),
        hand_in_progress: status.hand_in_progress,
    }
}

// ── SpectatorEvent construction ───────────────────────────────────────────────

impl SpectatorEvent {
    fn from_proto(event: &TableEvent) -> Self {
        SpectatorEvent {
            event_type: event_type_to_str(event.event_type).to_owned(),
            description: event.description.clone(),
            timestamp: event.timestamp,
            status: event.current_status.as_ref().map(table_status_to_snapshot),
        }
    }
}

// ── Route handlers ────────────────────────────────────────────────────────────

/// Serves the embedded HTML spectator page.
async fn handle_index() -> Html<&'static str> {
    Html(SPECTATOR_HTML)
}

/// SSE endpoint: streams [`TableEvent`]s as newline-delimited JSON to browser clients.
///
/// Uses the same broadcast-to-mpsc bridge pattern as the gRPC `stream_events` handler.
/// Lagged events are silently skipped for spectators — they are observers, not
/// game participants, so a gap in the log is acceptable.
async fn handle_sse(
    State(state): State<Arc<WebState>>,
) -> Sse<ReceiverStream<Result<Event, Infallible>>> {
    let mut broadcast_rx = state.event_tx.subscribe();
    let (mpsc_tx, mpsc_rx) = mpsc::channel::<Result<Event, Infallible>>(64);

    tokio::spawn(async move {
        loop {
            match broadcast_rx.recv().await {
                Ok(event) => {
                    let se = SpectatorEvent::from_proto(&event);
                    let data = serde_json::to_string(&se).unwrap_or_default();
                    if mpsc_tx.send(Ok(Event::default().data(data))).await.is_err() {
                        break; // client disconnected
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    // Silently skip lagged events for spectators.
                }
            }
        }
    });

    Sse::new(ReceiverStream::new(mpsc_rx)).keep_alive(KeepAlive::default())
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Starts the Axum HTTP server on the provided pre-bound listener.
///
/// Accepts a [`TcpListener`] rather than a `SocketAddr` so that tests can bind
/// port `0` and discover the OS-assigned port before calling this function.
///
/// # Errors
///
/// Returns `Err` if the underlying `axum::serve` call fails (e.g. broken pipe
/// on the listening socket).
pub async fn serve(
    listener: TcpListener,
    event_tx: broadcast::Sender<TableEvent>,
) -> Result<(), std::io::Error> {
    let state = Arc::new(WebState { event_tx });
    let app = Router::new()
        .route("/", get(handle_index))
        .route("/events", get(handle_sse))
        .with_state(state);
    axum::serve(listener, app).await
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use pkdealer_proto::dealer::SeatInfo;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // ── Mapping helpers ───────────────────────────────────────────────────────

    #[test]
    fn event_type_to_str_all_variants() {
        assert_eq!(
            event_type_to_str(EventType::HandStarted as i32),
            "hand_started"
        );
        assert_eq!(event_type_to_str(EventType::HandEnded as i32), "hand_ended");
        assert_eq!(
            event_type_to_str(EventType::PlayerAction as i32),
            "player_action"
        );
        assert_eq!(
            event_type_to_str(EventType::PlayerSeated as i32),
            "player_seated"
        );
        assert_eq!(
            event_type_to_str(EventType::StreetAdvanced as i32),
            "street_advanced"
        );
        assert_eq!(event_type_to_str(999), "unspecified");
    }

    #[test]
    fn street_to_str_all_variants() {
        assert_eq!(street_to_str(Street::Preflop as i32), "preflop");
        assert_eq!(street_to_str(Street::Flop as i32), "flop");
        assert_eq!(street_to_str(Street::Turn as i32), "turn");
        assert_eq!(street_to_str(Street::River as i32), "river");
        assert_eq!(street_to_str(999), "unspecified");
    }

    #[test]
    fn player_state_to_str_all_variants() {
        assert_eq!(
            player_state_to_str(ProtoPlayerState::YetToAct as i32),
            "yet_to_act"
        );
        assert_eq!(
            player_state_to_str(ProtoPlayerState::Folded as i32),
            "folded"
        );
        assert_eq!(
            player_state_to_str(ProtoPlayerState::AllIn as i32),
            "all_in"
        );
        assert_eq!(player_state_to_str(ProtoPlayerState::Blind as i32), "blind");
        assert_eq!(player_state_to_str(999), "unspecified");
    }

    // ── table_status_to_snapshot ──────────────────────────────────────────────

    #[test]
    fn table_status_to_snapshot_maps_seat_state() {
        let status = TableStatus {
            seats: vec![SeatInfo {
                seat_number: 2,
                player_name: "Alice".to_owned(),
                chips: 900,
                cards: String::new(),
                state: ProtoPlayerState::YetToAct as i32,
            }],
            board: "Ah Kd".to_owned(),
            pot: 150,
            current_street: Street::Flop as i32,
            ..Default::default()
        };
        let snap = table_status_to_snapshot(&status);
        assert_eq!(snap.seats.len(), 1);
        assert_eq!(snap.seats[0].player_name, "Alice");
        assert_eq!(snap.seats[0].chips, 900);
        assert_eq!(snap.seats[0].state, "yet_to_act");
        assert_eq!(snap.current_street, "flop");
        assert_eq!(snap.pot, 150);
        assert_eq!(snap.board, "Ah Kd");
    }

    #[test]
    fn table_status_to_snapshot_empty_seats() {
        let status = TableStatus::default();
        let snap = table_status_to_snapshot(&status);
        assert!(snap.seats.is_empty());
        assert_eq!(snap.current_street, "unspecified");
        assert_eq!(snap.pot, 0);
    }

    // ── SpectatorEvent ────────────────────────────────────────────────────────

    #[test]
    fn spectator_event_from_proto_without_status() {
        let event = TableEvent {
            timestamp: 42,
            event_type: EventType::PlayerSeated as i32,
            description: "Alice seated".to_owned(),
            current_status: None,
        };
        let se = SpectatorEvent::from_proto(&event);
        assert_eq!(se.event_type, "player_seated");
        assert_eq!(se.description, "Alice seated");
        assert_eq!(se.timestamp, 42);
        assert!(se.status.is_none());
    }

    #[test]
    fn spectator_event_from_proto_with_status() {
        let event = TableEvent {
            timestamp: 100,
            event_type: EventType::HandStarted as i32,
            description: "Hand started".to_owned(),
            current_status: Some(TableStatus {
                pot: 150,
                current_street: Street::Preflop as i32,
                hand_in_progress: true,
                ..Default::default()
            }),
        };
        let se = SpectatorEvent::from_proto(&event);
        assert_eq!(se.event_type, "hand_started");
        assert!(se.status.is_some());
        let snap = se.status.unwrap();
        assert_eq!(snap.pot, 150);
        assert_eq!(snap.current_street, "preflop");
        assert!(snap.hand_in_progress);
    }

    #[test]
    fn spectator_event_serializes_to_json() {
        let ev = SpectatorEvent {
            event_type: "hand_started".to_owned(),
            description: "Hand started".to_owned(),
            timestamp: 1,
            status: None,
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("hand_started"));
        assert!(json.contains("Hand started"));
        assert!(json.contains("timestamp"));
    }

    // ── HTTP server ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn serve_index_returns_200() -> Result<(), Box<dyn std::error::Error>> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (tx, _) = broadcast::channel::<TableEvent>(4);

        tokio::spawn(serve(listener, tx));
        // Allow the server a moment to start accepting connections.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Use a raw TCP connection to issue a minimal HTTP/1.1 GET — no reqwest dev-dep needed.
        let mut stream = tokio::net::TcpStream::connect(addr).await?;
        stream
            .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .await?;
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await?;
        let resp = String::from_utf8_lossy(&buf);
        assert!(
            resp.starts_with("HTTP/1.1 200"),
            "expected 200, got: {resp}"
        );
        assert!(resp.contains("PKDealer"), "expected 'PKDealer' in body");
        Ok(())
    }
}
