# Demo: EPIC-23a — Random Bot Agents

> Two autonomous bots connect via gRPC, seat themselves, and play continuous No-Limit Hold'em hands — no human required.

## Audience & framing

Engineering peers or a technical lead reviewing EPIC-23 progress. The angle is **infrastructure first**: show the shared agent framework (`pkdealer_agent_core`) in action before the interesting decision logic (rules-based and Claude agents) lands in 23b/23c.

---

## Prerequisites

- Repo checked out on branch `epic-23a`
- Rust toolchain present (`cargo --version`)
- Three terminal panes open side-by-side: service | alice bot | bob bot
- (Optional fourth pane or browser tabs for OTel stack)
- No other process listening on port 50051

---

## Setup (~ 4 minutes)

1. **Build both crates**
   ```bash
   cargo build --bin pkdealer_service --bin pkdealer_agent_random
   ```
   _Expected:_ `Finished` with no errors.
   _Talking point:_ One build step compiles the service, the core library, and the random agent.

2. **(Optional) Start the OTel stack**
   ```bash
   OTEL_SDK_DISABLED=false docker compose up -d
   ```
   _Expected:_ Four containers start (`pkdealer_service`, `otel-collector`, `jaeger`, `prometheus`, `grafana`).
   _Talking point:_ Same stack from EPIC-22 — we're reusing it untouched.

   > Skip this step if you don't need traces. Set `OTEL_SDK_DISABLED=true` in step 3 if skipping.

---

## The demo (~ 5 minutes)

### 1. Start the service

In **pane 1**:
```bash
cargo run --bin pkdealer_service
```
_Expected:_ `Dealer service listening on 0.0.0.0:50051`
_Talking point:_ The service is the autonomous game loop from EPIC-20 — it deals hands as soon as two players are seated.

> **Important:** If you ran agents before without restarting the service, ghost players remain seated and will block post-flop action. Always start with a fresh service process.

---

### 2. Seat alice

In **pane 2** (after the service is up):
```bash
cargo run --bin pkdealer_agent_random -- --name alice --chips 10000
```
_Expected:_ `[alice] seated at seat N with 10000 chips` (or similar service log in pane 1).
_Talking point:_ `run_agent` in `pkdealer_agent_core` handles connect, seat request, and token bookkeeping — the binary itself is 30 lines.

---

### 3. Seat bob — hands start immediately

In **pane 3**:
```bash
cargo run --bin pkdealer_agent_random -- --name bob --chips 10000
```
_Expected:_ Service log shows `HandStarted` events; alice and bob logs show decisions (Fold / Call / Raise / Check / Bet).
_Talking point:_ The moment a second player joins, the autonomous loop deals. No human had to click anything.

---

### 4. Watch the action in service logs (pane 1)

Point at the service terminal.

_Expected:_ Continuous stream of `PlayerAction` events — seat numbers, action types, amounts. Pot and chip totals visible per hand.
_Talking point:_ Chip conservation — the sum of all stacks stays at 20 000 throughout every hand.

---

### 5. (Optional) Inspect traces in Jaeger

```bash
open http://localhost:16686
```

Select service `pkdealer_service`, operation `act`, and click **Find Traces**.

_Expected:_ Each `act` RPC appears as a span with `poker.seat`, `poker.action_type`, and `poker.pot` attributes.
_Talking point:_ When the Claude agent (23c) lands, its `gen_ai.*` spans will nest here as parents of the `act` span — the OTel wiring is already in place.

---

### 6. (Optional) Grafana dashboard

```bash
open http://localhost:3001
```

Navigate to the pkdealer dashboard.

_Expected:_ `hands_played_total` counter incrementing; `pot_size` histogram updating.
_Talking point:_ Same dashboard from EPIC-22 — bots drive it just like humans would.

---

## What to highlight verbally

- **`PokerAgent` is the only contract.** One async `decide(&HandState) -> Decision` method. The random agent is 20 lines; the Claude agent will be ~80. Same runner, zero changes to infrastructure.
- **`run_agent` handles all transport.** Connect, seat, token metadata, event parsing, act dispatch — all in `pkdealer_agent_core`. Future agents inherit it for free.
- **Chip conservation is the correctness signal.** If stacks sum to exactly 20 000 at all times, the gRPC round-trip and action mapping are correct. No assertions needed in the demo — you can see it live.
- **23a unblocks 23b and 23c.** Rule-based and Claude agents are a `PokerAgent` impl each. The hard part (transport, state reconstruction, event loop) is already done.
- **Branch state: untracked.** These crates compile and test clean but aren't committed yet — this is the implementation in its raw form.

---

## Likely questions & answers

**Q: How does the agent know it's its turn?**
A: `StreamEvents` delivers every table event with a `current_status.next_to_act` field; `run_agent` skips events where that field doesn't match the agent's seat number.

**Q: What stops both agents from acting simultaneously?**
A: The service enforces turn order — an `Act` RPC from the wrong seat returns an error; `run_agent` surfaces that as `AgentError::Rpc`.

**Q: Can two agents of different types play each other?**
A: Yes — they're independent processes connected only by gRPC. A random agent and a future Claude agent are indistinguishable from the service's perspective.

**Q: Why `Decision` instead of `PlayerAction`?**
A: `pkdealer_proto` already generates a type called `PlayerAction`. Using `Decision` in the core library avoids a name collision and keeps the domain type free of proto dependencies.

**Q: How does seat resume work with agents?**
A: Pass `--client_secret <token>` to reconnect a named seat after a restart — the same EPIC-20 mechanism humans use. Agents can survive service restarts without losing their seat.

---

## Cleanup

```bash
# Kill agents (Ctrl-C in each terminal pane)
# Kill service (Ctrl-C in pane 1)

# If OTel stack was started:
docker compose down

# Return to main branch if needed:
git checkout main
```

---

## Troubleshooting

**Agent exits immediately with `Connect` error.**
The service isn't up yet. Start `pkdealer_service` first and wait for the `listening on` log line before starting agents.

**Agents seat at 3 and 4 instead of 0 and 1, then hang post-flop.**
Ghost players from previous runs are occupying the lower seats. The service does not evict players on disconnect. Kill agent processes (`pkill -f pkdealer_agent_random`), restart the service, then start agents fresh.

**`seat` rejected — table full.**
The service defaults to a maximum seat count. Kill any lingering agent processes (`pkill -f pkdealer_agent_random`) and restart.

**Grafana shows port 3000 not found.**
The docker-compose maps Grafana's internal port 3000 to host port **3001** — use `http://localhost:3001`, not 3000. The EPIC-23 spec has a typo.
