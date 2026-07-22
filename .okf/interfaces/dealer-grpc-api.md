---
type: API Contract
title: Dealer gRPC API (DealerService)
description: The protobuf/gRPC contract for the poker dealer — seating, hands, actions, queries, event streaming, and session export.
resource: https://github.com/ImperialBower/pkdealer/blob/main/proto/dealer.proto
tags: [grpc, protobuf, api]
timestamp: 2026-07-22T13:10:00Z
---

Defined in `proto/dealer.proto` (~420 lines), compiled by
[pkdealer_proto](/crates/pkdealer_proto.md), implemented by
[pkdealer_service](/crates/pkdealer_service.md), and consumed by the
[client](/crates/pkdealer_client.md) and all agents via
[pkdealer_agent_core](/crates/pkdealer_agent_core.md).

# Schema

| RPC | Purpose |
|---|---|
| `Ping` | Liveness check with client id |
| `SeatPlayer` / `SeatPlayerAt` | Seat a player (next free seat / specific seat) |
| `RemovePlayer` | Remove a player from the table |
| `StartHand` | Deal a new hand |
| `Act` | Submit a player action (bet / call / raise / fold) |
| `GetStatus` / `GetNextToAct` | Table state and turn order queries |
| `GetBoard` / `GetChips` / `GetPot` | Board cards, stacks, pot queries |
| `GetEventLog` / `GetTableConfig` | Event history and table configuration |
| `Rebuy` | Add chips to the caller's seat (`chips == 0` uses the service default) |
| `GetPlayerStats` | Per-player statistics |
| `StreamEvents` | Server-streaming table events (spectators, recorders) |
| `ExportSession` / `GetSessionInfo` | Recorded-session export (YAML/JSON) and metadata |

`StreamEvents` and `ExportSession` are what the arena recorder and
[pkdealer_costsim](/crates/pkdealer_costsim.md) build on.

# Citations

[1] [proto/dealer.proto](https://github.com/ImperialBower/pkdealer/blob/main/proto/dealer.proto)
