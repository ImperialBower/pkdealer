#!/usr/bin/env bash
# Launch the full pkdealer demo stack.
#
# Brings up the dealer service, OpenTelemetry collector, Jaeger, Prometheus,
# Grafana, and five agent containers (gto, lag, tag, random, ollama).
# The pkspectator UI runs separately from its own repo — see DEMO.md.

set -euo pipefail

# Soft check: warn if ollama isn't reachable. The rules and random agents
# still play even when ollama is down, so don't abort.
if ! curl -fsS http://localhost:11434/api/tags >/dev/null 2>&1; then
  cat <<'EOF'
⚠  Ollama not reachable at http://localhost:11434
   The ollama agent container will exit until ollama is running.
   The rules and random agents will still play.
   To enable the ollama agent:
     ollama serve
     ollama pull llama3.1

EOF
fi

echo "Starting pkdealer demo stack..."
docker compose up -d --build

echo ""
echo "Waiting for dealer service to accept connections on :50051..."
for _ in $(seq 1 60); do
  if nc -z localhost 50051 2>/dev/null; then
    break
  fi
  sleep 1
done

if ! nc -z localhost 50051 2>/dev/null; then
  echo "❌ Dealer service did not become reachable within 60s."
  echo "   Inspect logs: docker compose logs pkdealer_service"
  exit 1
fi

cat <<'EOF'

Demo is live:
  Jaeger:    http://localhost:16686
  Grafana:   http://localhost:3001
  Prom:      http://localhost:9090

To watch the table, run pkspectator from a separate checkout:
  cd ../pkspectator && cargo run
  open http://localhost:3000

Tail logs:    docker compose logs -f
Tear down:    docker compose down -v
EOF
