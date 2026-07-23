---
type: Runbook
title: Observability — OpenTelemetry stack
description: Bringing up and configuring the OTel collector, Jaeger, Prometheus, and Grafana alongside the dealer.
tags: [otel, jaeger, prometheus, grafana, runbook]
timestamp: 2026-07-22T13:10:00Z
---

The [dealer service](/crates/pkdealer_service.md) is OpenTelemetry-
instrumented, and the LLM agents
([claude](/crates/pkdealer_agent_claude.md),
[ollama](/crates/pkdealer_agent_ollama.md)) emit `gen_ai` spans so model
calls are traceable end-to-end.

# Steps

```sh
docker compose up -d --build   # dealer + collector + Jaeger + Prometheus + Grafana
make ddown                     # tear down (docker compose down -v)
```

Stack config lives in `ops/` (`otel-collector.yaml`, `prometheus.yml`,
`grafana/`). Full env-var reference and quickstart:
`crates/pkdealer_service/README.md`.

# Gotchas

* Set `OTEL_SDK_DISABLED=true` when running tests or bare `cargo run`
  without a collector — otherwise the SDK retries exports noisily.
