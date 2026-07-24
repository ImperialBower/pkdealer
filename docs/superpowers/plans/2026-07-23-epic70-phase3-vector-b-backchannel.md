# EPIC-70 Phase 3 — Vector-B Peer Backchannel Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a bot-to-bot peer backchannel (Vector B) so colluders can share hole cards through a broker the dealer never sees, behaviorally indistinguishable from Vector A — plus the additive `SeatInfo.player_id` proto/service change that gives agents a live identity.

**Architecture:** A new unconditional `pkdealer_backchannel` broker crate relays line-delimited-JSON `CardShare`s between colluding processes. Agent-side, a feature-gated `BackchannelClient` and the existing `SpectatorLeak` both implement one `PartnerCardSource` trait, so the `RulesAgent` decide path is byte-identical across channels (A/B equivalence enforced by the type system). The service exposes a per-seat UUID (`SeatInfo.player_id`, from its existing `seat_to_token` map) so agents can stamp and filter shares live.

**Tech Stack:** Rust 2024 workspace, tonic/prost gRPC (proto is prost-generated at build), tokio TCP (`net` + `io-util`), serde + serde_json, clap 4, bash (`bin/arena`), pkcore 0.3.1 (`Cards`).

**Spec:** `docs/superpowers/specs/2026-07-23-epic70-phase3-vector-b-backchannel-design.md`.

## Global Constraints

- **No git commands are run by the implementing agent — ever.** At each commit point, print the exact `git add … && git commit -m "…"` command for the user (Christoph) to run themselves, and wait. This is the user's global rule and overrides all skill defaults.
- The `SeatInfo.player_id` proto field + service population is an **additive, identity-only** change (a UUID is public, not a hole card). Do **not** touch `filter_cards` / card redaction. This is the deliberate amendment to EPIC-70's "proto untouched" non-goal.
- All new agent-side peer code is behind the existing `collusion` cargo feature on `pkdealer_agent_rules` and `pkdealer_agent_core`; with the feature off, every crate builds and all existing tests pass byte-identically.
- `pkdealer_backchannel` is a new **unconditional** workspace crate (a relay is not itself a cheat — mirrors `pkdealer_boss`).
- House rules (CLAUDE.md): no `unwrap()`/`expect()`/`panic!()` in library code (tests OK); every public item gets doc comments **with doc tests** (library crates only — a binary crate's doc tests don't run, use ```text```); unit-test names never start with `test_`; `#![warn(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]` at each crate root.
- Run tests with `OTEL_SDK_DISABLED=true`.
- `cargo --features` does not work with `--workspace` in a virtual workspace — always use `-p <crate> --features collusion`.
- Card-notation: `Cards::forgiving_from_str` parses index (`"Ah Kd"`) notation; the wire uses index notation.
- Enforced clippy gate is lib+bin: `cargo clippy -p <crate> -- -D warnings` (and `--features collusion` where relevant). `cargo test --workspace` runs tests without `cfg(test)`-gated feature code; the strict gate does not compile test modules.

## Documented deviations from the design sketch

Record these in the EPIC doc in Task 7 — they are deliberate:

1. **`SeatInfo.player_id` is a `string` UUID** (proto3 has no native UUID); empty string means "unknown" (older service, empty seat). Agents parse it to `Option<Uuid>`.
2. **The broker broadcasts to all *other* clients** rather than routing by pair — clients self-filter by partner UUID. Simplest correct relay; no pair config.
3. **`PartnerCardSource::partner_hole` takes `hand_no`, `my_cards`, and `partner_id`** even though `SpectatorLeak` ignores all three — a single signature keeps the decide path channel-agnostic (the point of the trait).

## File Structure

```
proto/dealer.proto                              + SeatInfo.player_id = 13
crates/pkdealer_service/src/main.rs             populate player_id at SeatInfo build (:601)
crates/pkdealer_backchannel/                    NEW unconditional crate
  Cargo.toml
  src/lib.rs                                    CardShare + Broker core (broadcast logic)
  src/main.rs                                   TCP broker binary
crates/pkdealer_agent_core/
  Cargo.toml                                    + uuid, serde, serde_json; tokio net+io-util
  src/hand_state.rs                             + SeatSnapshot.player_id: Option<Uuid>
  src/runner.rs                                 parse+thread player_id in seat_snapshot (:27)
  src/backchannel.rs                            NEW (feature collusion): CardShare + BackchannelClient
  src/lib.rs                                    + pub mod backchannel (feature-gated)
crates/pkdealer_agent_rules/
  Cargo.toml                                    (collusion feature already present)
  src/collude/mod.rs                            PartnerCardSource trait; Colluder.leak: Box<dyn>
  src/collude/spectator.rs                      impl PartnerCardSource for SpectatorLeak
  src/collude/backchannel_source.rs             NEW: impl PartnerCardSource for BackchannelClient
  src/main.rs                                   validate_collusion accepts peer; wire client
arena.toml                                      + channel field on teamed players
bin/arena                                       peer → broker service + env + flags
tests/arena_peer.sh                             NEW dry-run shell test
Cargo.toml                                      + crates/pkdealer_backchannel member
```

---

### Task 1: `SeatInfo.player_id` proto field + service population

**Files:**
- Modify: `proto/dealer.proto` (SeatInfo message), `crates/pkdealer_service/src/main.rs` (SeatInfo builder ~`:601`)
- Test: a service test asserting a seated player's `player_id` is populated

**Interfaces:**
- Produces: `dealer.SeatInfo.player_id: String` (prost-generated) — a stringified `Uuid`, empty when the seat has no token.

- [ ] **Step 1: Add the proto field.** In `proto/dealer.proto`, inside `message SeatInfo`, after `cost_micro_usd = 12;`:

```proto
  // Stable per-seat player UUID (the server's player token). Public identity,
  // not a hole card — never redacted. Empty when the seat is unoccupied or the
  // token is unknown. (EPIC-70 Phase 3.)
  string player_id = 13;
```

- [ ] **Step 2: Confirm the builder's scope.** Read `crates/pkdealer_service/src/main.rs` around the `seats.push(SeatInfo { … })` at ~`:601`. Confirm the enclosing method has `&self` access to `self.seat_to_token: HashMap<u8, Uuid>` (declared at `:355`) and that `i` is the seat index (`u8`). If the builder is a free function without the map, thread `seat_to_token` in as a parameter from the caller.

- [ ] **Step 3: Populate the field.** In the `SeatInfo { … }` literal, add (adjust `self.` per Step 2):

```rust
                    player_id: self
                        .seat_to_token
                        .get(&i)
                        .map(std::string::ToString::to_string)
                        .unwrap_or_default(),
```

- [ ] **Step 4: Write the failing test.** Mirror the nearest existing test that seats a player and inspects a `GetStatus`/`TableStatus` (search `crates/pkdealer_service/src/main.rs` tests for `seat_to_token` or a `map_to_seat_info`/status-building helper). Assert:

```rust
// After a player is seated at seat 0 with a known token, its SeatInfo carries
// that token as player_id (non-empty, parses as a Uuid).
let info = /* build/fetch the SeatInfo for the seated seat */;
assert!(!info.player_id.is_empty());
assert!(uuid::Uuid::parse_str(&info.player_id).is_ok());
```

If no status-building unit is directly testable, add the test against the smallest helper that constructs `SeatInfo` from server state (the same unit modified in Step 3), constructing a `ServerState` with one seated seat + a token in `seat_to_token`.

- [ ] **Step 5: Run to verify it fails** — `OTEL_SDK_DISABLED=true cargo test -p pkdealer_service player_id 2>&1 | tail -20`. Expected: FAIL (field empty / assertion) before Step 3 is in place, or a compile error if the test references `player_id` before regeneration. (Proto regenerates on `cargo build`.)

- [ ] **Step 6: Run to verify it passes** — `OTEL_SDK_DISABLED=true cargo test -p pkdealer_service player_id 2>&1 | tail -20` → PASS.

- [ ] **Step 7: Full service tests + clippy** — `OTEL_SDK_DISABLED=true cargo test -p pkdealer_service 2>&1 | tail -5` (all pre-existing pass) and `cargo clippy -p pkdealer_service -- -D warnings 2>&1 | tail -1` (clean).

- [ ] **Step 8: Note the identity-unification check.** Grep whether the recorder's `PlayerEntry.player_id` is the same UUID as `seat_to_token` (search the service's hand-recording path). Record the finding in the Task 7 doc notes — it's informational (Vector B works either way), not a blocker.

- [ ] **Step 9: Hand off commit**

```bash
git add proto/dealer.proto crates/pkdealer_service
git commit -m "feat(epic-70): expose per-seat player_id on SeatInfo (Phase 3 identity)"
```

---

### Task 2: Thread `player_id` onto `SeatSnapshot`

**Files:**
- Modify: `crates/pkdealer_agent_core/src/hand_state.rs` (struct + doc example + tests), `crates/pkdealer_agent_core/src/runner.rs` (`seat_snapshot` at `:27`), plus every other `SeatSnapshot { … }` full literal the compiler flags
- Modify: `crates/pkdealer_agent_core/Cargo.toml` (+ `uuid`)

**Interfaces:**
- Produces: `SeatSnapshot.player_id: Option<Uuid>` — the seat's stable UUID from `SeatInfo.player_id`; `None` when empty/unparseable.

- [ ] **Step 1: Add the `uuid` dep.** In `crates/pkdealer_agent_core/Cargo.toml` `[dependencies]`:

```toml
uuid = { version = "1.22", features = ["v4"] }
```

- [ ] **Step 2: Write the failing test.** Append to `hand_state.rs` tests:

```rust
#[test]
fn seat_snapshot_carries_player_id() {
    let id = uuid::Uuid::from_u128(0xC0FFEE);
    let s = SeatSnapshot {
        seat: 0,
        name: "alice".to_string(),
        chips: 100,
        bet: 0,
        is_active: true,
        player_id: Some(id),
    };
    assert_eq!(s.player_id, Some(id));
}
```

- [ ] **Step 3: Run to verify it fails** — `cargo test -p pkdealer_agent_core seat_snapshot_carries_player_id` → "missing field `player_id`".

- [ ] **Step 4: Add the field.** In `hand_state.rs`, after `is_active` in `struct SeatSnapshot`:

```rust
    /// Stable player UUID for this seat (from `SeatInfo.player_id`). `None`
    /// when the service does not report one. Lets a colluding agent stamp and
    /// filter peer card-shares by identity (EPIC-70 Phase 3).
    pub player_id: Option<Uuid>,
```

Add `use uuid::Uuid;` at the top of `hand_state.rs`. Update the `SeatSnapshot` doc example (add `player_id: None,`) and any full `SeatSnapshot { … }` literal in this file's tests.

- [ ] **Step 5: Populate in the runner.** In `runner.rs` `seat_snapshot` (`:27`), add to the `SeatSnapshot { … }` literal:

```rust
        player_id: s.player_id.parse().ok(),
```

(`s.player_id` is the proto `String`; `.parse::<Uuid>().ok()` → `Option<Uuid>`. Empty string → `Err` → `None`.)

- [ ] **Step 6: Fix every other construction site.** Run `cargo check --workspace --all-targets 2>&1 | grep -A1 "missing field \`player_id\`"` and add `player_id: None,` to each flagged full `SeatSnapshot { … }` literal (known: `pkdealer_agent_rules`/`_llm`/`_random` snapshot builders and their tests). Spread-syntax literals (`..seat(...)`) need no change.

- [ ] **Step 7: Full suite + clippy** — `OTEL_SDK_DISABLED=true cargo test --workspace 2>&1 | grep -E "^test result|FAILED" | tail` (all pass) and `cargo clippy --workspace -- -D warnings 2>&1 | tail -1` (clean).

- [ ] **Step 8: Hand off commit**

```bash
git add crates/pkdealer_agent_core crates/pkdealer_agent_rules crates/pkdealer_agent_llm crates/pkdealer_agent_random
git commit -m "feat(epic-70): thread SeatInfo.player_id onto SeatSnapshot (Phase 3)"
```

---

### Task 3: `pkdealer_backchannel` crate — `CardShare` + broker

**Files:**
- Create: `crates/pkdealer_backchannel/Cargo.toml`, `src/lib.rs`, `src/main.rs`
- Modify: root `Cargo.toml` (workspace members)

**Interfaces:**
- Produces:

```rust
// lib.rs
pub struct CardShare { pub hand_no: u32, pub seat: u8, pub player_id: Uuid, pub hole_cards: String } // serde
pub struct Broker { /* clients: broadcast senders */ }
impl Broker {
    pub fn new() -> Self;
    pub async fn serve(self, listener: tokio::net::TcpListener) -> std::io::Result<()>;
}
```

- [ ] **Step 1: Create the crate.** `crates/pkdealer_backchannel/Cargo.toml`:

```toml
[package]
name = "pkdealer_backchannel"
version.workspace = true
edition.workspace = true
authors.workspace = true
license.workspace = true
repository.workspace = true
description = "EPIC-70 Vector-B collusion backchannel broker: relays CardShares between colluding agents"
keywords = ["poker", "collusion", "broker", "epic-70"]
categories = ["games", "network-programming"]
rust-version = "1.85"

[lib]
path = "src/lib.rs"

[[bin]]
name = "pkdealer_backchannel"
path = "src/main.rs"

[dependencies]
tokio = { version = "1", features = ["macros", "rt-multi-thread", "net", "io-util", "sync"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
uuid = { version = "1.22", features = ["v4", "serde"] }
```

Add `"crates/pkdealer_backchannel",` to the root `Cargo.toml` `members` list (after `"crates/pkdealer_boss",`).

- [ ] **Step 2: Write the failing test** (in `src/lib.rs` `#[cfg(test)] mod tests`):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpStream;

    fn share(seat: u8, id: u128, cards: &str) -> CardShare {
        CardShare { hand_no: 7, seat, player_id: Uuid::from_u128(id), hole_cards: cards.to_string() }
    }

    #[tokio::test]
    async fn broker_broadcasts_to_others_not_sender() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { Broker::new().serve(listener).await.unwrap() });

        let mut a = TcpStream::connect(addr).await.unwrap();
        let b = TcpStream::connect(addr).await.unwrap();
        let mut b = BufReader::new(b);
        // Give the broker a moment to register both clients.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let line = serde_json::to_string(&share(0, 0xA1, "Ah Kd")).unwrap();
        a.write_all(format!("{line}\n").as_bytes()).await.unwrap();

        // B receives A's share.
        let mut got = String::new();
        b.read_line(&mut got).await.unwrap();
        let parsed: CardShare = serde_json::from_str(got.trim()).unwrap();
        assert_eq!(parsed.hole_cards, "Ah Kd");
        assert_eq!(parsed.player_id, Uuid::from_u128(0xA1));

        // A does NOT receive its own share back (nothing to read within a beat).
        let mut a = BufReader::new(a);
        let mut echo = String::new();
        let r = tokio::time::timeout(std::time::Duration::from_millis(100), a.read_line(&mut echo)).await;
        assert!(r.is_err() || echo.is_empty(), "sender must not receive its own share");
    }
}
```

- [ ] **Step 3: Run to verify it fails** — `cargo test -p pkdealer_backchannel 2>&1 | tail -10` → compile error (`Broker`/`CardShare` missing).

- [ ] **Step 4: Implement `src/lib.rs`:**

```rust
#![warn(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]
//! EPIC-70 Vector-B collusion backchannel: a broker that relays `CardShare`
//! lines between colluding agent processes. It is a dumb fan-out relay — it
//! broadcasts each received line to every *other* connected client and keeps
//! no state; clients filter for their partner. The dealer service never sees
//! these messages.

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use uuid::Uuid;

/// One colluder's hole cards for one hand, as shared over the backchannel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardShare {
    /// Dealer hand number the cards belong to.
    pub hand_no: u32,
    /// Sharer's seat.
    pub seat: u8,
    /// Sharer's stable player UUID.
    pub player_id: Uuid,
    /// Hole cards in index notation, e.g. `"Ah Kd"`.
    pub hole_cards: String,
}

/// A fan-out relay for `CardShare` lines.
pub struct Broker {
    tx: broadcast::Sender<(u64, String)>,
}

impl Default for Broker {
    fn default() -> Self {
        Self::new()
    }
}

impl Broker {
    /// Creates an idle broker.
    #[must_use]
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(1024);
        Self { tx }
    }

    /// Accepts connections until the listener errors, broadcasting each client's
    /// lines to every other client. Each connection is tagged with a unique id
    /// so the sender is excluded from its own broadcast.
    ///
    /// # Errors
    ///
    /// Returns the first fatal `accept` error.
    pub async fn serve(self, listener: TcpListener) -> std::io::Result<()> {
        let mut next_id: u64 = 0;
        loop {
            let (socket, _) = listener.accept().await?;
            let id = next_id;
            next_id += 1;
            let tx = self.tx.clone();
            let mut rx = self.tx.subscribe();
            tokio::spawn(async move {
                let (read_half, mut write_half) = socket.into_split();
                let mut lines = BufReader::new(read_half).lines();
                loop {
                    tokio::select! {
                        incoming = lines.next_line() => match incoming {
                            Ok(Some(line)) => { let _ = tx.send((id, line)); }
                            _ => break, // EOF or read error: drop this client
                        },
                        broadcasted = rx.recv() => match broadcasted {
                            Ok((from, line)) if from != id => {
                                if write_half.write_all(format!("{line}\n").as_bytes()).await.is_err() {
                                    break;
                                }
                            }
                            Ok(_) => {}                       // own message: skip
                            Err(broadcast::error::RecvError::Lagged(_)) => {}
                            Err(broadcast::error::RecvError::Closed) => break,
                        },
                    }
                }
            });
        }
    }
}
```

- [ ] **Step 5: Implement `src/main.rs`:**

```rust
#![warn(clippy::pedantic, clippy::unwrap_used, clippy::expect_used)]
//! `pkdealer_backchannel` broker binary (EPIC-70 Phase 3). Binds
//! `PKDEALER_BACKCHANNEL_BIND` (default `0.0.0.0:9099`) and relays `CardShare`
//! lines between colluding agents. Never contacts the dealer service.

use std::process::ExitCode;

use pkdealer_backchannel::Broker;

#[tokio::main]
async fn main() -> ExitCode {
    let bind = std::env::var("PKDEALER_BACKCHANNEL_BIND").unwrap_or_else(|_| "0.0.0.0:9099".to_string());
    let listener = match tokio::net::TcpListener::bind(&bind).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("pkdealer_backchannel: bind {bind} failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!("pkdealer_backchannel: relaying on {bind}");
    if let Err(e) = Broker::new().serve(listener).await {
        eprintln!("pkdealer_backchannel: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
```

- [ ] **Step 6: Run tests + doc tests** — `OTEL_SDK_DISABLED=true cargo test -p pkdealer_backchannel 2>&1 | tail -10` → PASS. Add a doc test on `CardShare` (serde round-trip) and `Broker::new` so `cargo test --doc -p pkdealer_backchannel` passes.

- [ ] **Step 7: Clippy** — `cargo clippy -p pkdealer_backchannel -- -D warnings 2>&1 | tail -1` → clean.

- [ ] **Step 8: Hand off commit**

```bash
git add Cargo.toml crates/pkdealer_backchannel
git commit -m "feat(epic-70): pkdealer_backchannel broker crate + CardShare (Phase 3a)"
```

---

### Task 4: `BackchannelClient` in `pkdealer_agent_core`

**Files:**
- Create: `crates/pkdealer_agent_core/src/backchannel.rs`
- Modify: `crates/pkdealer_agent_core/src/lib.rs` (feature-gated `pub mod backchannel;`), `crates/pkdealer_agent_core/Cargo.toml` (+ serde/serde_json/pkcore; tokio net/io-util)

**Interfaces:**
- Consumes: `pkdealer_backchannel::CardShare` shape (re-declared here to avoid a dependency cycle — agent_core must not depend on the broker binary crate; define an identical `CardShare` locally, feature-gated).
- Produces:

```rust
pub struct BackchannelClient { /* write half + Arc<Mutex<HashMap<(Uuid,u32), Cards>>> */ }
impl BackchannelClient {
    pub async fn connect(addr: &str) -> Result<Self, String>;
    pub async fn publish(&self, share: CardShare);
    pub async fn partner_cards(&self, partner_id: Uuid, hand_no: u32) -> Option<Cards>;
}
pub struct CardShare { pub hand_no: u32, pub seat: u8, pub player_id: Uuid, pub hole_cards: String }
```

- [ ] **Step 1: Add deps + feature wiring.** In `crates/pkdealer_agent_core/Cargo.toml`: add `"net", "io-util", "sync"` to the tokio features; add under `[dependencies]`:

```toml
pkcore = { version = "0.3.1" }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

(`uuid` was added in Task 2.) In `src/lib.rs`, add:

```rust
#[cfg(feature = "collusion")]
pub mod backchannel;
```

- [ ] **Step 2: Write the failing test** (in `backchannel.rs`, gated on the `collusion` feature; starts a real broker over loopback):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use pkcore::Forgiving;

    async fn broker() -> String {
        // Minimal inline broadcast relay mirroring pkdealer_backchannel::Broker,
        // so agent_core needs no dependency on the broker binary crate.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        tokio::spawn(async move {
            let (tx, _) = tokio::sync::broadcast::channel::<(u64, String)>(256);
            let mut id = 0u64;
            loop {
                let (sock, _) = listener.accept().await.unwrap();
                let (me, txc, mut rx) = (id, tx.clone(), tx.subscribe());
                id += 1;
                tokio::spawn(async move {
                    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
                    let (r, mut w) = sock.into_split();
                    let mut lines = BufReader::new(r).lines();
                    loop {
                        tokio::select! {
                            l = lines.next_line() => match l { Ok(Some(l)) => { let _ = txc.send((me, l)); }, _ => break },
                            b = rx.recv() => if let Ok((from, l)) = b { if from != me { let _ = w.write_all(format!("{l}\n").as_bytes()).await; } },
                        }
                    }
                });
            }
        });
        addr
    }

    #[tokio::test]
    async fn backchannel_matches_shares_by_hand_no() {
        let addr = broker().await;
        let trudy_id = uuid::Uuid::from_u128(0xA2);
        let mallory = BackchannelClient::connect(&addr).await.unwrap();
        let trudy = BackchannelClient::connect(&addr).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        trudy.publish(CardShare { hand_no: 7, seat: 1, player_id: trudy_id, hole_cards: "Qs Qc".into() }).await;
        // Let the share traverse the broker + reader task.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        assert_eq!(
            mallory.partner_cards(trudy_id, 7).await,
            Some(pkcore::cards::Cards::forgiving_from_str("Qs Qc"))
        );
        // Wrong hand → None (no cross-hand contamination).
        assert!(mallory.partner_cards(trudy_id, 8).await.is_none());
    }
}
```

- [ ] **Step 3: Run to verify it fails** — `cargo test -p pkdealer_agent_core --features collusion backchannel 2>&1 | tail -10` → compile error.

- [ ] **Step 4: Implement `backchannel.rs`:**

```rust
//! Vector B (`BackchannelClient`): shares this agent's hole cards with its
//! colluding partner over a broker the dealer never sees, and reads the
//! partner's, matched by `hand_no`. Best-effort — a missing/late partner share
//! yields `None`, and the agent decides honestly that turn (same graceful
//! degradation as Vector A).

use std::collections::HashMap;
use std::sync::Arc;

use pkcore::Forgiving;
use pkcore::cards::Cards;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::Mutex;
use uuid::Uuid;

/// One colluder's hole cards for one hand (wire-identical to the broker's).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardShare {
    /// Dealer hand number the cards belong to.
    pub hand_no: u32,
    /// Sharer's seat.
    pub seat: u8,
    /// Sharer's stable player UUID.
    pub player_id: Uuid,
    /// Hole cards in index notation.
    pub hole_cards: String,
}

type Buffer = Arc<Mutex<HashMap<(Uuid, u32), String>>>;

/// A colluder's connection to the backchannel broker.
pub struct BackchannelClient {
    write: Mutex<OwnedWriteHalf>,
    buffer: Buffer,
}

impl BackchannelClient {
    /// Dials the broker and spawns a background reader that buffers incoming
    /// shares by `(player_id, hand_no)`.
    ///
    /// # Errors
    ///
    /// Returns a human-readable message when the broker is unreachable.
    pub async fn connect(addr: &str) -> Result<Self, String> {
        let stream = TcpStream::connect(addr)
            .await
            .map_err(|e| format!("backchannel connect {addr} failed: {e}"))?;
        let (read, write) = stream.into_split();
        let buffer: Buffer = Arc::new(Mutex::new(HashMap::new()));
        let reader_buffer = Arc::clone(&buffer);
        tokio::spawn(async move {
            let mut lines = BufReader::new(read).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(share) = serde_json::from_str::<CardShare>(&line) {
                    reader_buffer
                        .lock()
                        .await
                        .insert((share.player_id, share.hand_no), share.hole_cards);
                }
            }
        });
        Ok(Self {
            write: Mutex::new(write),
            buffer,
        })
    }

    /// Publishes this agent's cards for the current hand. Best-effort.
    pub async fn publish(&self, share: CardShare) {
        if let Ok(mut line) = serde_json::to_string(&share) {
            line.push('\n');
            let _ = self.write.lock().await.write_all(line.as_bytes()).await;
        }
    }

    /// The partner's cards for `hand_no`, or `None` if not yet received.
    pub async fn partner_cards(&self, partner_id: Uuid, hand_no: u32) -> Option<Cards> {
        self.buffer
            .lock()
            .await
            .get(&(partner_id, hand_no))
            .map(|s| Cards::forgiving_from_str(s))
    }
}
```

- [ ] **Step 5: Run tests** — `OTEL_SDK_DISABLED=true cargo test -p pkdealer_agent_core --features collusion backchannel 2>&1 | tail -10` → PASS. Also `cargo test -p pkdealer_agent_core 2>&1 | tail -3` (feature off, untouched).

- [ ] **Step 6: Clippy both ways** — `cargo clippy -p pkdealer_agent_core -- -D warnings` and `cargo clippy -p pkdealer_agent_core --features collusion -- -D warnings` → clean. (If `CardShare`/`BackchannelClient` are unused with the wiring not yet in place, they are consumed in Task 5 within the same crate feature; if a transient `dead_code` fires under `--features collusion`, add `#![cfg_attr(not(test), allow(dead_code))]` atop `backchannel.rs` with a "wired in Task 5" note and remove it in Task 5.)

- [ ] **Step 7: Hand off commit**

```bash
git add crates/pkdealer_agent_core
git commit -m "feat(epic-70): BackchannelClient peer card-sharing client (Phase 3a/3b)"
```

---

### Task 5: `PartnerCardSource` trait + Colluder refactor + peer wiring

**Files:**
- Modify: `crates/pkdealer_agent_rules/src/collude/mod.rs` (trait; `Colluder.leak: Box<dyn>`), `crates/pkdealer_agent_rules/src/collude/spectator.rs` (impl), `crates/pkdealer_agent_rules/src/main.rs` (`validate_collusion`, `Colluder`, `choose`, `main` wiring)
- Create: `crates/pkdealer_agent_rules/src/collude/backchannel_source.rs`
- Modify: `crates/pkdealer_agent_rules/Cargo.toml` (+ `pkdealer_agent_core` already a dep; ensure `uuid` present)

**Interfaces:**
- Consumes: `SpectatorLeak` (Task 1 Phase 1), `pkdealer_agent_core::backchannel::{BackchannelClient, CardShare}` (Task 4), `SeatSnapshot.player_id` (Task 2).
- Produces:

```rust
#[async_trait::async_trait]
pub trait PartnerCardSource: Send + Sync {
    async fn partner_hole(&self, hand_no: u32, my_cards: &Cards, partner_id: Uuid) -> Option<Cards>;
}
```

- [ ] **Step 1: Write the failing test** (in `collude/mod.rs` or a new `collude/tests.rs`, feature-gated) — the in-process A/B equivalence:

```rust
#[cfg(test)]
mod ab_equivalence {
    use super::*;
    use pkcore::Forgiving;
    use pkcore::cards::Cards;

    struct Fixed(Cards);
    #[async_trait::async_trait]
    impl PartnerCardSource for Fixed {
        async fn partner_hole(&self, _h: u32, _m: &Cards, _p: uuid::Uuid) -> Option<Cards> {
            Some(self.0.clone())
        }
    }

    #[tokio::test]
    async fn two_sources_same_partner_hole_are_interchangeable() {
        // Any two PartnerCardSources returning the same cards feed apply_style
        // identically — the decision path is channel-agnostic.
        let cards = Cards::forgiving_from_str("As Ac");
        let a = Fixed(cards.clone());
        let b = Fixed(cards.clone());
        let ha = a.partner_hole(7, &cards, uuid::Uuid::from_u128(1)).await;
        let hb = b.partner_hole(7, &cards, uuid::Uuid::from_u128(1)).await;
        assert_eq!(ha, hb);
    }
}
```

- [ ] **Step 2: Run to verify it fails** — `cargo test -p pkdealer_agent_rules --features collusion ab_equivalence 2>&1 | tail -10` → `PartnerCardSource` not found.

- [ ] **Step 3: Define the trait** in `collude/mod.rs`:

```rust
use pkcore::cards::Cards;
use uuid::Uuid;

/// A source of a colluding partner's hole cards for the current hand. Both
/// Vector A (`SpectatorLeak`) and Vector B (`BackchannelClient`) implement it,
/// so the decide path is byte-identical across channels — the Boss catches the
/// behavior, not the channel.
#[async_trait::async_trait]
pub trait PartnerCardSource: Send + Sync {
    /// The partner's hole cards this hand, or `None` (decide honestly).
    async fn partner_hole(&self, hand_no: u32, my_cards: &Cards, partner_id: Uuid) -> Option<Cards>;
}

pub mod backchannel_source;
```

- [ ] **Step 4: Impl for `SpectatorLeak`** (in `spectator.rs`) — it reads the partner live and ignores the extra args:

```rust
#[async_trait::async_trait]
impl crate::collude::PartnerCardSource for SpectatorLeak {
    async fn partner_hole(&self, _hand_no: u32, _my_cards: &pkcore::cards::Cards, _partner_id: uuid::Uuid) -> Option<pkcore::cards::Cards> {
        self.partner_hole().await
    }
}
```

(Rename the existing inherent `SpectatorLeak::partner_hole(&self)` if the names collide — keep the inherent one as `read_partner_live` and call it from the trait impl.)

- [ ] **Step 5: Impl for `BackchannelClient`** (in `collude/backchannel_source.rs`):

```rust
//! Vector B adapter: makes `BackchannelClient` a `PartnerCardSource` by
//! publishing this agent's cards and reading the partner's, matched by hand.

use pkcore::cards::Cards;
use pkdealer_agent_core::backchannel::{BackchannelClient, CardShare};
use uuid::Uuid;

pub struct PeerSource {
    pub client: BackchannelClient,
    pub my_seat: u8,
    pub my_id: Uuid,
}

#[async_trait::async_trait]
impl crate::collude::PartnerCardSource for PeerSource {
    async fn partner_hole(&self, hand_no: u32, my_cards: &Cards, partner_id: Uuid) -> Option<Cards> {
        self.client
            .publish(CardShare {
                hand_no,
                seat: self.my_seat,
                player_id: self.my_id,
                hole_cards: my_cards.to_string(),
            })
            .await;
        self.client.partner_cards(partner_id, hand_no).await
    }
}
```

- [ ] **Step 6: Refactor `Colluder`** (in `main.rs`): change `leak: collude::spectator::SpectatorLeak` to `source: Box<dyn collude::PartnerCardSource>`. Update `choose` to resolve `partner_id` from the snapshot and call the trait:

```rust
        #[cfg(feature = "collusion")]
        if let Some(colluder) = &self.collusion {
            let partner = state.stacks.iter().find(|s| s.name == colluder.config.partner);
            if let (Some(partner), Some(my)) = (
                partner,
                state.stacks.iter().find(|s| s.seat == state.seat),
            ) {
                if let (Some(partner_id), Some(_my_id)) = (partner.player_id, my.player_id) {
                    let my_cards = pkcore::cards::Cards::forgiving_from_str(&state.hole_cards);
                    if let Some(partner_hole) = colluder
                        .source
                        .partner_hole(state.hand_no, &my_cards, partner_id)
                        .await
                    {
                        return collude::strategy::apply_style(
                            colluder.config.style, base, snapshot, partner.seat, &partner_hole,
                        );
                    }
                }
            }
        }
```

- [ ] **Step 7: Accept `peer` in `validate_collusion`** — remove the peer rejection; return the channel so `main` can build the right source. Keep the spectator-token requirement for `Spectator` only.

- [ ] **Step 8: Wire the source in `main`** — for `Spectator`, build `SpectatorLeak` (as today) boxed; for `Peer`, read `PKDEALER_BACKCHANNEL` (env, e.g. via a new `--backchannel` arg defaulting to `std::env::var`), `BackchannelClient::connect`, and box a `PeerSource { client, my_seat, my_id }`. `my_seat`/`my_id` come from the agent's own seat once known — if not known at construction, store them lazily in `PeerSource` as `Option` and fill on first decide, OR resolve from the first status snapshot. (Simplest: resolve `my_seat`/`my_id` inside `choose` from `state.stacks` where `s.seat == state.seat`, and pass them into a `partner_hole` that also takes `my_seat`/`my_id` — adjust the trait signature to include them if cleaner. Pick one and keep it consistent.)

- [ ] **Step 9: Remove the transient `dead_code` allow** on `CollusionChannel::Peer` (now constructed) from `collude/mod.rs`.

- [ ] **Step 10: Run tests both ways + clippy** — `OTEL_SDK_DISABLED=true cargo test -p pkdealer_agent_rules --features collusion` and without; `cargo clippy -p pkdealer_agent_rules --features collusion -- -D warnings` and without. All green/clean.

- [ ] **Step 11: Hand off commit**

```bash
git add crates/pkdealer_agent_rules
git commit -m "feat(epic-70): PartnerCardSource trait unifies Vector A/B; accept peer channel (Phase 3b/3c)"
```

---

### Task 6: `arena.toml` `channel` + `bin/arena` peer wiring

**Files:**
- Modify: `arena.toml` (schema doc + a peer example), `bin/arena` (emit broker service + peer flags)
- Create: `tests/arena_peer.sh` (executable)

**Interfaces:**
- Produces: a compose override where a `channel = peer` team's agents carry `--collusion-channel peer` + `PKDEALER_BACKCHANNEL=backchannel:9099`, plus a single `backchannel` service (image `pkdealer/backchannel:latest`, `BIN_NAME: pkdealer_backchannel`).

- [ ] **Step 1: Write the failing shell test** — `tests/arena_peer.sh`:

```bash
#!/usr/bin/env bash
# EPIC-70 Phase 3: channel=peer → backchannel broker + peer flags (dry-run only).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
fail() { echo "FAIL: $*" >&2; exit 1; }

out="$(./bin/arena --dry-run mallory trudy gto)"
override="$(sed -n 's/^Override file: //p' <<<"$out")"
[[ -f "$override" ]] || fail "no override"

grep -q -- '"--collusion-channel", "peer"' "$override" || fail "peer channel flag missing"
grep -q -- 'PKDEALER_BACKCHANNEL: backchannel:9099' "$override" || fail "backchannel env missing"
grep -q -- 'agent_backchannel:' "$override" || fail "broker service missing"
grep -q -- 'BIN_NAME: pkdealer_backchannel' "$override" || fail "broker bin missing"

out2="$(./bin/arena --dry-run gto lag)"
override2="$(sed -n 's/^Override file: //p' <<<"$out2")"
grep -q -- 'backchannel' "$override2" && fail "team-less lineup emitted a broker"

echo "OK: arena peer expansion"
```

Precondition: set `mallory`/`trudy`'s team to `channel = "peer"` (Step 2). Run `chmod +x tests/arena_peer.sh && ./tests/arena_peer.sh` → FAIL (no peer wiring yet).

- [ ] **Step 2: `arena.toml`** — document `channel` in the header block (next to `style`), and set the existing `mallory`/`trudy` team to peer (or add a note showing how):

```toml
# `channel` (optional, teamed seats) — card-leak vector: spectator (Vector A,
#   default) | peer (Vector B, needs the backchannel broker; bin/arena adds it).
```

Add `channel = "peer"` to `[players.mallory]` and `[players.trudy]` (they already share `team = "A"`).

- [ ] **Step 3: `bin/arena`** — in the collusion expansion, capture each team's `channel` (default `spectator`) alongside `style`; in `emit_service`, when a rules colluder's channel is `peer`, emit `--collusion-channel peer` (instead of `spectator`) and `PKDEALER_BACKCHANNEL: backchannel:9099` in `environment`. After the agent loop, if any peer team exists, append one broker service:

```bash
emit_backchannel() {
  {
    printf '  agent_backchannel:\n'
    printf '    image: pkdealer/backchannel:latest\n'
    printf '    build:\n      context: .\n      dockerfile: Dockerfile.agent\n      args:\n'
    printf '        BIN_NAME: pkdealer_backchannel\n'
    printf '    environment:\n      PKDEALER_BACKCHANNEL_BIND: 0.0.0.0:9099\n'
    printf '    restart: unless-stopped\n'
  } >> "$OVERRIDE"
}
```

Track a `have_peer` flag while emitting agents; call `emit_backchannel` once if set. (Reuse `Dockerfile.agent`; `pkdealer_backchannel` is a normal bin target, no `FEATURES` needed.)

- [ ] **Step 4: Run the shell test** — `./tests/arena_peer.sh` → `OK: arena peer expansion`. Re-run `./tests/arena_team.sh` (spectator path) to confirm no regression.

- [ ] **Step 5: Hand off commit**

```bash
git add arena.toml bin/arena tests/arena_peer.sh
git commit -m "feat(epic-70): arena channel=peer → backchannel broker wiring (Phase 3b)"
```

---

### Task 7: Docs, EPIC status, OKF, verification

**Files:**
- Modify: `docs/EPIC-70_Collusion_and_Cheat_Detection.md`, `docs/BACKLOG.md`, `.okf/interfaces/dealer-grpc-api.md`, `.okf/crates/index.md`, `.okf/log.md`
- Create: `.okf/crates/pkdealer_backchannel.md`

- [ ] **Step 1: EPIC doc.** Flip the Vector-B Status row to **Complete** (3a/3b); check work items 3a–3c. **Amend the Context non-goal**: replace "The proto is untouched" with a note that Phase 3 adds the additive, identity-only `SeatInfo.player_id` (rationale: public identity, not a card; redaction unchanged). Add a Phase-3 entry to the Implementation corrigendum (the 3 deviations above + the non-goal amendment + the identity-unification finding from Task 1 Step 8). Update the Phase status summary table (Phase 3 → Complete, note the live-docker A/B signature check still deferred).

- [ ] **Step 2: OKF.** Create `.okf/crates/pkdealer_backchannel.md` (mirror `pkdealer_boss.md` frontmatter; `type: Rust Crate`, tags `[binary, library, collusion, backchannel, epic-70]`, today's timestamp). Add it to `.okf/crates/index.md` under "Collusion detection". Note the `SeatInfo.player_id` field in `.okf/interfaces/dealer-grpc-api.md` and bump its `timestamp`. Append a dated `.okf/log.md` entry. Validate: `uv run "…/okf_validate.py" .okf --strict`.

- [ ] **Step 3: BACKLOG.** Update the EPIC-70 row → "Phases 0–3 done; Phases 4–5 deferred".

- [ ] **Step 4: Full verification sweep:**

```bash
cargo build --workspace
cargo build -p pkdealer_agent_rules --features collusion
cargo clippy --workspace -- -D warnings
cargo clippy -p pkdealer_agent_rules --features collusion -- -D warnings
cargo clippy -p pkdealer_agent_core --features collusion -- -D warnings
OTEL_SDK_DISABLED=true cargo test --workspace
OTEL_SDK_DISABLED=true cargo test -p pkdealer_agent_rules --features collusion
OTEL_SDK_DISABLED=true cargo test -p pkdealer_agent_core --features collusion
cargo test --doc -p pkdealer_backchannel
./tests/arena_team.sh
./tests/arena_peer.sh
```

Expected: all green.

- [ ] **Step 5: Report exit-criteria status honestly** — criterion 5 (A/B equivalence) now met *in-process* (`vector_a_and_b_same_decision` / `two_sources_same_partner_hole_are_interchangeable`); the live-session behavioral-signature equivalence remains deferred to the manual docker checklist.

- [ ] **Step 6: Hand off final commit**

```bash
git add docs/EPIC-70_Collusion_and_Cheat_Detection.md docs/BACKLOG.md .okf
git commit -m "docs(epic-70): mark Phase 3 delivered; OKF + backlog refresh"
```

---

## Self-review

1. **Spec coverage:** §1 proto/service → T1; §2 agent UUID → T2; §3 backchannel/broker → T3 (broker) + T4 (client); §4 PartnerCardSource unification → T5; §5 arena wiring → T6; §6 testing → distributed across T3–T6 (broker broadcast, hand-no matching, absent→None, A/B decision, service player_id); §7 deferred → documented in T7 Step 5; §8 docs → T7. All covered.
2. **Placeholder scan:** T5 Steps 8 leaves a genuine implementation choice (where `my_seat`/`my_id` are resolved) explicit with two concrete options — not a placeholder but a decision the implementer makes and keeps consistent; flagged as such. All code steps carry real code.
3. **Type consistency:** `CardShare` fields identical in T3 (broker) and T4 (client, re-declared to avoid a binary-crate dependency cycle — deliberate, noted). `PartnerCardSource::partner_hole(hand_no, my_cards, partner_id)` signature identical in T5 Steps 3/4/5. `SeatSnapshot.player_id: Option<Uuid>` consistent T2→T5. `BackchannelClient::{connect,publish,partner_cards}` consistent T4→T5.
