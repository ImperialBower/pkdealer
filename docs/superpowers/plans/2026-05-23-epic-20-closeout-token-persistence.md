# EPIC-20 Close-out: Seat Resume via `client_secret` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an opt-in `client_secret` to `SeatPlayer` / `SeatPlayerAt` so a crashed agent process can re-attach to its existing seat (with the same `x-player-token`) on restart — the last EPIC-20 prerequisite for EPIC-23 bot agents.

**Architecture:** Add a `secret_to_token: HashMap<String, Uuid>` to `TableState`. When `SeatPlayer{,At}` is called with a known secret, the service returns the existing seat and token instead of allocating a new one and sets `resumed: true` in the response. Removing a player clears the entry. Service-side only; no on-disk persistence — service restart wipes the map, by design.

**Tech Stack:** Rust, tonic gRPC, prost (proto codegen), pkcore `PokerSession`, tokio. No new dependencies.

---

## Context for the executing agent (read this first)

**Everything else EPIC-20 listed as "Planned" is already done.** Before writing this plan we confirmed by reading the code:

- `pkcore = "0.0.48"` is already pinned (`Cargo.toml`); the doc's "0.0.39 → latest" item is stale.
- `TableState` already wraps `PokerSession`, not `Dealer`. There is no `unsafe impl Send` anywhere in `crates/pkdealer_service/src/main.rs`.
- The `Act` handler at `crates/pkdealer_service/src/main.rs:722` already runs the full auto-advance loop via `SessionStep::{PlayerToAct, StreetAdvanced, HandComplete}`, including auto-`end_hand` + `HandEnded` emission.
- The proto comment on `Act` (`proto/dealer.proto:28`) already promises "Streets and hand resolution advance automatically".

**So the work is exactly:** proto fields + map + handler changes + tests + doc update. Nothing else.

### Cross-references to existing code

| What you need to touch | Where it lives |
|---|---|
| Proto messages `SeatPlayerRequest` / `SeatPlayerResponse` / `SeatPlayerAtRequest` / `SeatPlayerAtResponse` | `proto/dealer.proto:109-133` |
| `TableState` struct | `crates/pkdealer_service/src/main.rs:108-124` |
| `DealerService::new` initializer | `crates/pkdealer_service/src/main.rs:184-212` |
| `seat_player` handler | `crates/pkdealer_service/src/main.rs:478-537` |
| `seat_player_at` handler | `crates/pkdealer_service/src/main.rs:539-596` |
| `remove_player` token cleanup | `crates/pkdealer_service/src/main.rs:633-635` (extend to clear `secret_to_token`) |
| Existing e2e test pattern (process spawn + client) | `crates/pkdealer_service/tests/e2e_two_players.rs` |

### Project rules the executing agent must follow

From `CLAUDE.md`:
- **Never** use `unwrap()` / `expect()` / `panic!()` in library/handler code. Tests may.
- Every public function gets doc comments **with a runnable `# Examples`** doc test.
- Every public function gets a unit test.
- Doc tests must compile (`cargo test --doc`).

From `MEMORY.md`:
- **Never** prefix Rust test fn names with `test_`. Use `resume_with_secret_returns_same_seat`, not `test_resume_with_secret`.

From `~/.claude/CLAUDE.md` (global):
- The user runs all state-changing git commands themselves. Commit steps below show the exact command; **suggest it to the user — do not run `git add` / `git commit` yourself**.

### Design decisions (already settled — do not re-litigate)

| Decision | Resolution |
|---|---|
| What's the resume key? | A client-chosen `client_secret` string. Opaque to the service. |
| What if resume is requested but the seat was vacated (`RemovePlayer`)? | The `secret_to_token` entry was cleared on removal; the secret is unknown → fresh seat path. |
| What if the same secret is presented while the seat is still occupied? | Return the same `player_token` and `seat`, `resumed: true`. Re-attach is idempotent. |
| What about `chips` on resume? | **Ignore the request's `chips` field on the resume path.** The existing seat keeps its current chip stack — re-attaching mid-session must not magically reset the stack to 10,000. |
| Does the service persist the map across restarts? | **No.** In-memory only. Service restart = clean slate. |
| Two processes with the same secret? | Both get the same token. Last writer wins on `Act`. Documented as a footgun. |
| Auth on resume? | None beyond holding the secret. This is a local-demo feature, not a security boundary. |

---

## File Map

| File | Action | Responsibility |
|---|---|---|
| `proto/dealer.proto` | Modify | Add `client_secret` to two request messages, `resumed` to two response messages |
| `crates/pkdealer_service/src/main.rs` | Modify | Add `secret_to_token` field; resume branch in two handlers; cleanup in `remove_player` |
| `crates/pkdealer_service/tests/e2e_seat_resume.rs` | Create | New e2e test file covering the resume contract |
| `docs/EPIC-20_Autonomous_Game_Loop.md` | Modify | Flip stale "Planned" rows to **Complete**; add a "Seat resume via client_secret" row marked **Complete** |
| `DEVLOG.md` | Modify | Append an EPIC-20 close-out entry |

---

## Task 1: Add `client_secret` and `resumed` to the proto

**Files:**
- Modify: `proto/dealer.proto` (messages at lines 109–133)

- [ ] **Step 1: Edit `SeatPlayerRequest` to add `client_secret`**

Open `proto/dealer.proto` and replace the existing `SeatPlayerRequest` block with:

```protobuf
message SeatPlayerRequest {
  string name  = 1;
  uint32 chips = 2;  // 0 → server default (10 000). Ignored on resume.
  // Optional. If a previous SeatPlayer/SeatPlayerAt call from this client
  // supplied the same client_secret AND that seat has not been removed,
  // the existing seat and player_token are returned and `resumed` is true.
  // Ignored if empty. Opaque to the service.
  string client_secret = 3;
}
```

- [ ] **Step 2: Edit `SeatPlayerResponse` to add `resumed`**

Replace the existing `SeatPlayerResponse` block with:

```protobuf
message SeatPlayerResponse {
  oneof result {
    uint32 seat_number = 1;
    string error       = 2;
  }
  // UUID to use as x-player-token metadata on future RPCs.
  string player_token = 3;
  // True if this response re-attached to an existing seat via client_secret;
  // false if a fresh seat was allocated.
  bool   resumed      = 4;
}
```

- [ ] **Step 3: Edit `SeatPlayerAtRequest` to add `client_secret`**

Replace the existing `SeatPlayerAtRequest` block with:

```protobuf
message SeatPlayerAtRequest {
  uint32 seat  = 1;
  string name  = 2;
  uint32 chips = 3;  // 0 → server default. Ignored on resume.
  // See SeatPlayerRequest.client_secret. On resume, the `seat` field is
  // validated to match the seat the secret was originally bound to; mismatch
  // returns an error.
  string client_secret = 4;
}
```

- [ ] **Step 4: Edit `SeatPlayerAtResponse` to add `resumed`**

Replace the existing `SeatPlayerAtResponse` block with:

```protobuf
message SeatPlayerAtResponse {
  oneof result {
    bool   success = 1;
    string error   = 2;
  }
  string player_token = 3;
  bool   resumed      = 4;
}
```

- [ ] **Step 5: Verify codegen still compiles**

Run: `cargo build -p pkdealer_proto -p pkdealer_service`
Expected: clean build. The generated structs in `pkdealer_proto` now have `client_secret: String` and `resumed: bool` fields populated by their `Default::default()` (empty string / `false`), so existing call sites still compile.

- [ ] **Step 6: Suggest commit to the user**

Tell the user:

```bash
git add proto/dealer.proto && git commit -m "EPIC-20: add client_secret and resumed to seat-player protos"
```

---

## Task 2: Add `secret_to_token` field to `TableState`

**Files:**
- Modify: `crates/pkdealer_service/src/main.rs:108-124` (`TableState` struct)
- Modify: `crates/pkdealer_service/src/main.rs:195-203` (initializer in `DealerService::new`)

- [ ] **Step 1: Add the field to `TableState`**

In `crates/pkdealer_service/src/main.rs`, find the `TableState` struct (starts at line 109) and add this field after `seat_to_token`:

```rust
    /// Maps client-chosen secrets → player UUID tokens for seat resume.
    /// See `SeatPlayerRequest.client_secret`. Entries are removed when a
    /// seat is vacated via `remove_player`. Empty when no resume hints are
    /// in play.
    secret_to_token: HashMap<String, Uuid>,
```

- [ ] **Step 2: Initialize the field in `DealerService::new`**

In the `TableState { ... }` literal inside `DealerService::new` (around line 195), add `secret_to_token: HashMap::new(),` after the `seat_to_token: HashMap::new(),` line.

- [ ] **Step 3: Verify the service still compiles**

Run: `cargo build -p pkdealer_service`
Expected: clean build.

- [ ] **Step 4: Suggest commit to the user**

```bash
git add crates/pkdealer_service/src/main.rs && git commit -m "EPIC-20: add secret_to_token map to TableState"
```

---

## Task 3: Write failing e2e tests for the resume contract

**Files:**
- Create: `crates/pkdealer_service/tests/e2e_seat_resume.rs`

> **Why a new test file:** the existing `e2e_two_players.rs` is focused on action flow with two seated players. Resume tests need different setup (single client, two `SeatPlayer` calls). Keeping them separate keeps each file under ~400 lines and named for its concern.

- [ ] **Step 1: Create the new test file with full content**

Create `crates/pkdealer_service/tests/e2e_seat_resume.rs` with the content below. The file follows the spawn-binary harness pattern from `e2e_two_players.rs` (lines 1-188 are the reusable bits).

```rust
//! End-to-end tests for seat resume via `client_secret`.
//!
//! Covers EPIC-20's last close-out item: a crashed agent process must be
//! able to re-attach to its seat by presenting the same `client_secret`
//! and receive the same `player_token` and `seat_number`.

use std::{
    io,
    net::TcpListener,
    path::PathBuf,
    process::{Child, Command},
    time::{Duration, Instant},
};

use pkdealer_proto::dealer::{
    SeatPlayerAtRequest, SeatPlayerRequest, dealer_service_client::DealerServiceClient,
    seat_player_at_response, seat_player_response,
};
use tonic::Request;

// ── process helpers (mirrors e2e_two_players.rs) ─────────────────────────────

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
        if start.elapsed() > timeout {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn spawn_service() -> (ChildProcessGuard, String) {
    let port = reserve_local_port().expect("port");
    let bin = service_bin_path().expect("service bin");
    let child = Command::new(&bin)
        .env("PKDEALER_PORT", port.to_string())
        .env("OTEL_SDK_DISABLED", "true")
        .spawn()
        .expect("spawn pkdealer_service");
    let guard = ChildProcessGuard { child };
    let endpoint = format!("http://127.0.0.1:{port}");
    assert!(
        wait_for_service_ready(&endpoint, Duration::from_secs(5)).await,
        "service did not become ready",
    );
    (guard, endpoint)
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resume_with_secret_returns_same_seat_and_token() {
    let (_guard, endpoint) = spawn_service().await;
    let mut client = DealerServiceClient::connect(endpoint)
        .await
        .expect("connect");

    // First call: fresh seat allocation.
    let first = client
        .seat_player(Request::new(SeatPlayerRequest {
            name: "alice".to_owned(),
            chips: 10_000,
            client_secret: "alice-secret-abc".to_owned(),
        }))
        .await
        .expect("first seat")
        .into_inner();
    let first_seat = match first.result {
        Some(seat_player_response::Result::SeatNumber(s)) => s,
        other => panic!("expected SeatNumber, got {other:?}"),
    };
    assert!(!first.player_token.is_empty(), "first token populated");
    assert!(!first.resumed, "first call is not a resume");

    // Second call with the same secret: same seat + same token, resumed=true.
    let second = client
        .seat_player(Request::new(SeatPlayerRequest {
            name: "alice".to_owned(),
            chips: 99_999,                       // should be ignored on resume
            client_secret: "alice-secret-abc".to_owned(),
        }))
        .await
        .expect("second seat")
        .into_inner();
    let second_seat = match second.result {
        Some(seat_player_response::Result::SeatNumber(s)) => s,
        other => panic!("expected SeatNumber, got {other:?}"),
    };

    assert_eq!(first_seat, second_seat, "same seat on resume");
    assert_eq!(
        first.player_token, second.player_token,
        "same token on resume",
    );
    assert!(second.resumed, "second call is a resume");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seat_without_secret_does_not_register_for_resume() {
    let (_guard, endpoint) = spawn_service().await;
    let mut client = DealerServiceClient::connect(endpoint)
        .await
        .expect("connect");

    // No secret on first call → no resume binding.
    let first = client
        .seat_player(Request::new(SeatPlayerRequest {
            name: "bob".to_owned(),
            chips: 10_000,
            client_secret: String::new(),
        }))
        .await
        .expect("first seat")
        .into_inner();
    assert!(!first.resumed);
    let first_token = first.player_token.clone();

    // Second call with an unrelated (and previously-unseen) secret → fresh
    // seat, different token, resumed=false.
    let second = client
        .seat_player(Request::new(SeatPlayerRequest {
            name: "bob2".to_owned(),
            chips: 10_000,
            client_secret: "never-used-before".to_owned(),
        }))
        .await
        .expect("second seat")
        .into_inner();
    assert!(!second.resumed);
    assert_ne!(first_token, second.player_token, "different tokens");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seat_player_at_resume_returns_same_seat() {
    let (_guard, endpoint) = spawn_service().await;
    let mut client = DealerServiceClient::connect(endpoint)
        .await
        .expect("connect");

    let first = client
        .seat_player_at(Request::new(SeatPlayerAtRequest {
            seat: 3,
            name: "carol".to_owned(),
            chips: 10_000,
            client_secret: "carol-secret".to_owned(),
        }))
        .await
        .expect("first seat_at")
        .into_inner();
    assert!(matches!(
        first.result,
        Some(seat_player_at_response::Result::Success(true)),
    ));
    assert!(!first.resumed);
    let first_token = first.player_token.clone();

    let second = client
        .seat_player_at(Request::new(SeatPlayerAtRequest {
            seat: 3,
            name: "carol".to_owned(),
            chips: 10_000,
            client_secret: "carol-secret".to_owned(),
        }))
        .await
        .expect("second seat_at")
        .into_inner();
    assert!(matches!(
        second.result,
        Some(seat_player_at_response::Result::Success(true)),
    ));
    assert!(second.resumed);
    assert_eq!(first_token, second.player_token);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seat_player_at_resume_wrong_seat_returns_error() {
    let (_guard, endpoint) = spawn_service().await;
    let mut client = DealerServiceClient::connect(endpoint)
        .await
        .expect("connect");

    // Bind secret to seat 2.
    let _ = client
        .seat_player_at(Request::new(SeatPlayerAtRequest {
            seat: 2,
            name: "dave".to_owned(),
            chips: 10_000,
            client_secret: "dave-secret".to_owned(),
        }))
        .await
        .expect("first seat_at");

    // Try to resume the same secret at a *different* seat → error.
    let bad = client
        .seat_player_at(Request::new(SeatPlayerAtRequest {
            seat: 5,
            name: "dave".to_owned(),
            chips: 10_000,
            client_secret: "dave-secret".to_owned(),
        }))
        .await
        .expect("second seat_at")
        .into_inner();
    match bad.result {
        Some(seat_player_at_response::Result::Error(msg)) => {
            assert!(
                msg.contains("seat 2") || msg.contains("mismatch"),
                "error should mention the original seat or mismatch; got: {msg}",
            );
        }
        other => panic!("expected error, got {other:?}"),
    }
    assert!(!bad.resumed);
}
```

- [ ] **Step 2: Run the new test file — they must fail (no implementation yet)**

Run: `cargo test -p pkdealer_service --test e2e_seat_resume`
Expected: all four tests **fail**, with assertion failures like `assert!(second.resumed, "second call is a resume")` failing because `resumed` is always `false` until Task 4. (Compilation must succeed — only the assertions fail. If compilation fails, fix the test file before proceeding.)

- [ ] **Step 3: Suggest commit to the user**

```bash
git add crates/pkdealer_service/tests/e2e_seat_resume.rs && git commit -m "EPIC-20: failing e2e tests for client_secret resume"
```

---

## Task 4: Implement resume in `seat_player`

**Files:**
- Modify: `crates/pkdealer_service/src/main.rs:478-537` (`seat_player` handler)

- [ ] **Step 1: Replace the body of `seat_player` with the resume-aware version**

Locate the `async fn seat_player(...)` impl. Replace its body (from after the signature to just before `async fn seat_player_at`) with:

```rust
        let req = request.into_inner();
        let chips = if req.chips == 0 {
            DEFAULT_CHIPS
        } else {
            req.chips as usize
        };

        let (response_result, player_token, resumed, maybe_event) = {
            let mut guard = self.lock()?;

            // Resume path: secret already bound to a token → return existing seat.
            if !req.client_secret.is_empty()
                && let Some(&token) = guard.secret_to_token.get(&req.client_secret)
                && let Some(&seat) = guard.token_to_seat.get(&token)
            {
                (
                    seat_player_response::Result::SeatNumber(u32::from(seat)),
                    token.to_string(),
                    true,
                    None,
                )
            } else {
                let size = guard.session.table.seats.size();
                let seat_num = (0..size).find(|&i| {
                    guard
                        .session
                        .table
                        .seats
                        .get_seat(i)
                        .is_some_and(SeatNoCell::is_empty)
                });
                match seat_num {
                    Some(i) => {
                        if let Some(s) = guard.session.table.seats.get_seat_mut(i) {
                            s.player = PlayerNoCell::new_with_chips(req.name.clone(), chips);
                        }
                        let token = Uuid::new_v4();
                        guard.token_to_seat.insert(token, i);
                        guard.seat_to_token.insert(i, token);
                        if !req.client_secret.is_empty() {
                            guard
                                .secret_to_token
                                .insert(req.client_secret.clone(), token);
                        }
                        let status =
                            Self::build_table_status(&guard.session, CardVisibility::Spectator);
                        let event = (
                            EventType::PlayerSeated,
                            format!("Player seated at seat {i}"),
                            status,
                        );
                        (
                            seat_player_response::Result::SeatNumber(u32::from(i)),
                            token.to_string(),
                            false,
                            Some(event),
                        )
                    }
                    None => (
                        seat_player_response::Result::Error("no empty seat available".to_owned()),
                        String::new(),
                        false,
                        None,
                    ),
                }
            }
        };

        if let Some((et, desc, status)) = maybe_event {
            self.emit_event(et, desc, status);
        }

        Ok(Response::new(SeatPlayerResponse {
            result: Some(response_result),
            player_token,
            resumed,
        }))
```

- [ ] **Step 2: Run the seat_player tests — `resume_with_secret_returns_same_seat_and_token` and `seat_without_secret_does_not_register_for_resume` must now pass**

Run: `cargo test -p pkdealer_service --test e2e_seat_resume resume_with_secret_returns_same_seat_and_token seat_without_secret_does_not_register_for_resume`
Expected: both pass. The two `seat_player_at_*` tests still fail (not implemented yet) — that's OK.

- [ ] **Step 3: Suggest commit to the user**

```bash
git add crates/pkdealer_service/src/main.rs && git commit -m "EPIC-20: implement client_secret resume in seat_player"
```

---

## Task 5: Implement resume in `seat_player_at`

**Files:**
- Modify: `crates/pkdealer_service/src/main.rs:539-596` (`seat_player_at` handler)

> **Resume contract difference:** `seat_player_at` requires the requested `seat` to match the seat the secret was originally bound to. Mismatch is an error, not a silent re-seat — agents using `SeatPlayerAt` are explicit about which seat they want.

- [ ] **Step 1: Add a private `fresh_seat_at_inner` helper to `impl DealerService`**

In `crates/pkdealer_service/src/main.rs`, locate the existing `impl DealerService { ... }` block (the one with `fn new`, `fn lock`, `build_table_status`, etc. — *not* the `impl DealerServiceTrait for DealerService`). Add this associated function inside that block. Take `&mut TableState` rather than `&mut MutexGuard<...>` so callers can pass `&mut *guard` with the conventional `Deref` idiom.

```rust
    /// Allocates a fresh seat for `SeatPlayerAt`, returning the response-tuple
    /// shape used by `seat_player_at`. Registers the player token and
    /// (when non-empty) the `client_secret → token` binding.
    fn fresh_seat_at_inner(
        state: &mut TableState,
        requested_seat: u8,
        name: &str,
        chips: usize,
        client_secret: &str,
    ) -> (
        seat_player_at_response::Result,
        String,
        bool,
        Option<(EventType, String, TableStatus)>,
    ) {
        let is_available = state
            .session
            .table
            .seats
            .get_seat(requested_seat)
            .is_some_and(SeatNoCell::is_empty);
        if !is_available {
            let msg = format!("seat {requested_seat} is occupied or does not exist");
            return (
                seat_player_at_response::Result::Error(msg),
                String::new(),
                false,
                None,
            );
        }
        if let Some(s) = state.session.table.seats.get_seat_mut(requested_seat) {
            s.player = PlayerNoCell::new_with_chips(name.to_owned(), chips);
        }
        let token = Uuid::new_v4();
        state.token_to_seat.insert(token, requested_seat);
        state.seat_to_token.insert(requested_seat, token);
        if !client_secret.is_empty() {
            state
                .secret_to_token
                .insert(client_secret.to_owned(), token);
        }
        let status = Self::build_table_status(&state.session, CardVisibility::Spectator);
        let event = (
            EventType::PlayerSeated,
            format!("Player seated at seat {requested_seat}"),
            status,
        );
        (
            seat_player_at_response::Result::Success(true),
            token.to_string(),
            false,
            Some(event),
        )
    }
```

Verify it compiles in isolation:

Run: `cargo build -p pkdealer_service`
Expected: clean build. The helper is unused at this point — Rust will not warn because it's a private fn on a type the crate uses publicly, but be ready to use it in step 2 immediately.

- [ ] **Step 2: Replace the body of `seat_player_at` with the resume-aware version**

Locate `async fn seat_player_at` (around line 539). Replace its body with:

```rust
        let req = request.into_inner();
        let chips = if req.chips == 0 {
            DEFAULT_CHIPS
        } else {
            req.chips as usize
        };
        #[allow(clippy::cast_possible_truncation)]
        let requested_seat = req.seat as u8;

        let (response_result, player_token, resumed, maybe_event) = {
            let mut guard = self.lock()?;

            // Resume path: secret bound to a token → require its seat to match.
            if !req.client_secret.is_empty()
                && let Some(&token) = guard.secret_to_token.get(&req.client_secret)
            {
                if let Some(&bound_seat) = guard.token_to_seat.get(&token) {
                    if bound_seat == requested_seat {
                        (
                            seat_player_at_response::Result::Success(true),
                            token.to_string(),
                            true,
                            None,
                        )
                    } else {
                        (
                            seat_player_at_response::Result::Error(format!(
                                "client_secret already bound to seat {bound_seat}; \
                                 requested seat {requested_seat} mismatch",
                            )),
                            String::new(),
                            false,
                            None,
                        )
                    }
                } else {
                    // Secret known but token no longer maps to a seat — stale entry.
                    // Drop it and fall through to fresh-seat allocation.
                    guard.secret_to_token.remove(&req.client_secret);
                    Self::fresh_seat_at_inner(
                        &mut *guard,
                        requested_seat,
                        &req.name,
                        chips,
                        &req.client_secret,
                    )
                }
            } else {
                Self::fresh_seat_at_inner(
                    &mut *guard,
                    requested_seat,
                    &req.name,
                    chips,
                    &req.client_secret,
                )
            }
        };

        if let Some((et, desc, status)) = maybe_event {
            self.emit_event(et, desc, status);
        }

        Ok(Response::new(SeatPlayerAtResponse {
            result: Some(response_result),
            player_token,
            resumed,
        }))
```

- [ ] **Step 3: Run all four resume tests — all must pass**

Run: `cargo test -p pkdealer_service --test e2e_seat_resume`
Expected: 4 passed, 0 failed.

- [ ] **Step 4: Run the full service test suite to confirm no regressions**

Run: `cargo test -p pkdealer_service`
Expected: all previously-passing tests still pass; 4 new tests added.

- [ ] **Step 5: Suggest commit to the user**

```bash
git add crates/pkdealer_service/src/main.rs && git commit -m "EPIC-20: implement client_secret resume in seat_player_at"
```

---

## Task 6: Clear `secret_to_token` in `remove_player`

**Files:**
- Modify: `crates/pkdealer_service/src/main.rs:632-635` (the existing token cleanup block in `remove_player`)

> **Why:** without this, a removed seat's secret stays in the map. A later `SeatPlayer` with that secret would resume to a token whose `token_to_seat` entry was already removed — producing inconsistent state. We want removal to fully forget the player.

- [ ] **Step 1: Add a failing test for cleanup**

Append this test to `crates/pkdealer_service/tests/e2e_seat_resume.rs`:

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remove_player_clears_secret_binding() {
    use pkdealer_proto::dealer::RemovePlayerRequest;

    let (_guard, endpoint) = spawn_service().await;
    let mut client = DealerServiceClient::connect(endpoint)
        .await
        .expect("connect");

    // Seat with a secret.
    let first = client
        .seat_player(Request::new(SeatPlayerRequest {
            name: "evict".to_owned(),
            chips: 10_000,
            client_secret: "evict-secret".to_owned(),
        }))
        .await
        .expect("first seat")
        .into_inner();
    let seat = match first.result {
        Some(seat_player_response::Result::SeatNumber(s)) => s,
        other => panic!("expected SeatNumber, got {other:?}"),
    };
    let first_token = first.player_token.clone();

    // Remove that seat.
    let _ = client
        .remove_player(Request::new(RemovePlayerRequest { seat }))
        .await
        .expect("remove");

    // Reseating with the same secret must allocate a *fresh* seat and token —
    // the previous binding was cleaned up.
    let second = client
        .seat_player(Request::new(SeatPlayerRequest {
            name: "evict-again".to_owned(),
            chips: 10_000,
            client_secret: "evict-secret".to_owned(),
        }))
        .await
        .expect("second seat")
        .into_inner();
    assert!(!second.resumed, "secret should have been cleared on removal");
    assert_ne!(first_token, second.player_token, "fresh token after removal");
}
```

- [ ] **Step 2: Run it and confirm it fails**

Run: `cargo test -p pkdealer_service --test e2e_seat_resume remove_player_clears_secret_binding`
Expected: fails because `resumed` is `true` (the secret entry is still present in `secret_to_token`).

- [ ] **Step 3: Update the token cleanup block in `remove_player`**

In `crates/pkdealer_service/src/main.rs`, find the existing block (around line 632):

```rust
            // Clean up the auth token for the removed seat.
            if let Some(uuid) = guard.seat_to_token.remove(&seat) {
                guard.token_to_seat.remove(&uuid);
            }
```

Replace with:

```rust
            // Clean up the auth token AND any resume binding for the removed seat.
            if let Some(uuid) = guard.seat_to_token.remove(&seat) {
                guard.token_to_seat.remove(&uuid);
                guard.secret_to_token.retain(|_, t| *t != uuid);
            }
```

- [ ] **Step 4: Run the failing test — must now pass**

Run: `cargo test -p pkdealer_service --test e2e_seat_resume remove_player_clears_secret_binding`
Expected: pass.

- [ ] **Step 5: Run the full service suite**

Run: `cargo test -p pkdealer_service`
Expected: all green.

- [ ] **Step 6: Suggest commit to the user**

```bash
git add crates/pkdealer_service/src/main.rs crates/pkdealer_service/tests/e2e_seat_resume.rs && git commit -m "EPIC-20: clear secret_to_token on remove_player"
```

---

## Task 7: Update doc comments on `TableState.secret_to_token` and the handlers

**Files:**
- Modify: `crates/pkdealer_service/src/main.rs` — doc comments on the two handlers and the new field

> **Why:** CLAUDE.md requires `# Examples` doc tests on public functions. These handlers are not public (they live behind the trait impl), so they require *good* doc comments but not doc tests. The new field also needs a clear comment.

- [ ] **Step 1: Update the `seat_player` doc comment**

Add (or extend, if one exists) the doc block immediately above `async fn seat_player`:

```rust
    /// Seats a new player at the next available seat, OR re-attaches an
    /// existing seat if `client_secret` matches a previous call.
    ///
    /// # Resume contract
    ///
    /// If `request.client_secret` is non-empty and already bound to a live
    /// token, the response carries the original `seat_number` and
    /// `player_token` with `resumed = true`. The `name` and `chips` fields
    /// in the request are **ignored on the resume path** — the seat keeps
    /// its existing player handle and chip stack.
    ///
    /// Resume bindings are dropped automatically when the seat is removed
    /// via [`Self::remove_player`].
    ///
    /// # Errors
    ///
    /// Returns `Ok` with an error variant in the `result` oneof when no
    /// empty seat is available and no resume binding matched. The handler
    /// itself does not return `tonic::Status::Err`.
```

- [ ] **Step 2: Update the `seat_player_at` doc comment similarly**

```rust
    /// Seats a new player at a specific seat, OR re-attaches if
    /// `client_secret` matches a previous call to either seat-player RPC.
    ///
    /// # Resume contract
    ///
    /// Same as [`Self::seat_player`], with one extra constraint: the
    /// requested `seat` must equal the seat the secret was originally
    /// bound to. Mismatch returns an error in the response (not a
    /// `tonic::Status`); fresh-seat allocation is not attempted in the
    /// mismatch case so the caller learns about the conflict.
```

- [ ] **Step 3: Build to confirm doc comments parse and no warnings appear**

Run: `cargo build -p pkdealer_service && cargo clippy -p pkdealer_service -- -D warnings`
Expected: clean build, no clippy warnings.

- [ ] **Step 4: Suggest commit to the user**

```bash
git add crates/pkdealer_service/src/main.rs && git commit -m "EPIC-20: document seat resume contract on handlers"
```

---

## Task 8: Flip EPIC-20 status doc to Complete and add the resume row

**Files:**
- Modify: `docs/EPIC-20_Autonomous_Game_Loop.md`

- [ ] **Step 1: Update the status table**

In `docs/EPIC-20_Autonomous_Game_Loop.md`, find the status table near the top. Replace the rows that say "Planned" with the actual current state. The new table should read (the executing agent may need to read the current file to preserve adjacent unchanged rows):

```markdown
| Component | Status |
|---|---|
| All 16 RPC handlers implemented | **Complete** |
| UUID-based auth (`x-player-token`) | **Complete** |
| Event broadcast via `tokio::sync::broadcast` | **Complete** |
| E2E tests (`e2e_ping`, `e2e_two_players`, `e2e_seat_resume`) | **Complete** |
| pkcore dependency update (0.0.39 → 0.0.48) | **Complete** |
| Migrate from `Dealer` → `PokerSession` (removes `unsafe impl Send`) | **Complete** |
| Auto-advance street when betting is complete | **Complete** |
| Auto-end hand when game is over | **Complete** |
| Seat resume via `client_secret` (EPIC-23 prerequisite) | **Complete** |
```

- [ ] **Step 2: Append a "Close-out" subsection at the end of the doc**

Append after the existing content:

```markdown

---

## Close-out (2026-05-23)

The status table above was updated retroactively: by the time we audited
the code for EPIC-23 prerequisites we found `PokerSession`, auto-advance,
and auto-end-hand had already been implemented in earlier commits. The
only EPIC-20 item not yet shipped was the **seat resume via
`client_secret`** mechanism added in this round of work — a required
prerequisite for EPIC-23 bot agents to survive process restarts without
losing their seats.

See `docs/superpowers/plans/2026-05-23-epic-20-closeout-token-persistence.md`
for the implementation plan and `crates/pkdealer_service/tests/e2e_seat_resume.rs`
for the contract tests.
```

- [ ] **Step 3: Suggest commit to the user**

```bash
git add docs/EPIC-20_Autonomous_Game_Loop.md && git commit -m "EPIC-20: flip status to Complete; document close-out"
```

---

## Task 9: DEVLOG entry

**Files:**
- Modify: `DEVLOG.md`

- [ ] **Step 1: Append a new entry at the end of `DEVLOG.md`**

```markdown

---

## EPIC-20 close-out — Seat resume via `client_secret` (2026-05-23)

**Status: ✅ Complete**

### What was added

A client-chosen `client_secret` string can now be passed on `SeatPlayer` and
`SeatPlayerAt`. If the same secret is seen on a later call (and the seat
has not been removed), the service returns the original seat number and
`x-player-token` and sets `resumed: true` in the response. This lets a
crashed agent process re-attach to its seat on restart without losing
chips or identity.

### Why this matters

The service is the only authoritative state for an agent's seat — when an
agent crashed, the proto offered no way to re-claim the seat without
either (a) calling `RemovePlayer` and starting fresh (losing chips) or
(b) the user manually arranging seat numbers. Both are unacceptable for
EPIC-23's autonomous bot agents.

### Scope of changes

- `proto/dealer.proto`: added `client_secret` (request) and `resumed`
  (response) to both `SeatPlayer*` message pairs.
- `crates/pkdealer_service/src/main.rs`: added `secret_to_token` map to
  `TableState`; resume branches in both handlers; cleanup in
  `remove_player`.
- `crates/pkdealer_service/tests/e2e_seat_resume.rs`: 5 new e2e tests
  covering happy path, no-secret path, `SeatPlayerAt` happy path, seat
  mismatch, and removal cleanup.

### Out of scope (deliberately)

- **Service-side persistence to disk.** Service restart wipes the map.
- **Authentication of the secret.** Anyone with the file can take over
  the seat; acceptable for local-demo scope.
- **Action timeout / auto-fold.** A bot that crashes mid-turn does not
  block the table only because nothing forces it to act yet. If demos
  surface this, add it in a future EPIC.

### Sets up

EPIC-23 (`pkdealer_agent_core`) can now ship a `load_or_create_secret`
helper that persists a per-agent UUID to `~/.pkdealer/agents/<name>.secret`
and threads it into every `SeatPlayer` call. See
`docs/EPIC-23_Bot_Agents.md`.
```

- [ ] **Step 2: Suggest final commit to the user**

```bash
git add DEVLOG.md && git commit -m "EPIC-20: DEVLOG entry for seat resume close-out"
```

---

## Final verification

- [ ] **Step 1: Full workspace test pass**

Run: `cargo test --workspace`
Expected: all green. Doc tests included.

- [ ] **Step 2: Clippy clean**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 3: Format check**

Run: `cargo fmt --all -- --check`
Expected: no diff.

- [ ] **Step 4: Manual smoke (optional but recommended)**

```bash
cargo run -p pkdealer_service &  # starts on default port
# In another shell, use grpcurl or the example client to:
#   1. Call SeatPlayer with client_secret="test-x"
#   2. Call SeatPlayer with client_secret="test-x" again
#   3. Confirm the second response has resumed=true and same seat/token
kill %1
```

- [ ] **Step 5: Report back**

When all steps are checked, tell the user:

> "EPIC-20 close-out complete. 5 new e2e tests passing, full workspace test+clippy+fmt green. Ready to start EPIC-23a (`pkdealer_agent_core` + random agent). Want the next plan written?"
