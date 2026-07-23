---
type: Reference
title: EPIC design docs
description: Design work is specified as numbered EPIC documents under docs/; closed EPICs carry a -CLOSED suffix.
resource: https://github.com/ImperialBower/pkdealer/tree/main/docs
tags: [docs, epics, process]
timestamp: 2026-07-22T15:30:00Z
---

All substantial design work is written up as a numbered `EPIC-NN_*.md`
document in `docs/` before implementation: Context, Status table, Design
sketches, phased Work Items, and a Verification block. Suffix conventions:
`-CLOSED` (done), `-INC` (incomplete/paused). `docs/BACKLOG.md` and
`docs/TECHNICAL_DEBT.md` track outstanding work.

Numbering is namespaced across the pkcore repo family in ten-blocks, registered
in `pkcore/ROADMAP.md` ("EPIC Numbering Policy"): pkdealer owns 40–45 (block
full) and 70–78. Check that registry before allocating a new number.

# Notable EPICs (as of 2026-07-22)

| EPIC | Topic | Status |
|---|---|---|
| 20–24 | Autonomous game loop, spectator, OTel, bot agents, demo | closed |
| 25 | Arena recorder & session export | active |
| 40 | Local LLM backend | incomplete |
| 41 | Reproducible scenarios | active |
| 42 | Dynamic arena runner (`./bin/arena`) | incomplete |
| 43 | PokerBench integration (cross-repo with `pkcore`) | active |
| 44 | Token accounting & cost simulation ([pricing](/crates/pkdealer_pricing.md), [costsim](/crates/pkdealer_costsim.md)) | incomplete |
| 45 | Bot evaluation format | active |
| 70 | Collusion simulation & cheat detection (consolidated 2026-07-22 from the retired pkdealer EPIC-46 + the "sentinel" draft) | active |

# Citations

[1] [docs/ directory](https://github.com/ImperialBower/pkdealer/tree/main/docs)
