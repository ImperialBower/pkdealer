# pkdealer_agent_ollama

Poker agent powered by [Ollama](https://ollama.com/) — a locally-served LLM.
Mirrors `pkdealer_agent_claude` in shape: the same poker prompt is built, the
same response parsing is applied, only the HTTP backend differs.

The agent connects to the `pkdealer` gRPC service, takes a seat, and asks the
configured Ollama model what action to take on every turn it owns.

## One-time setup

Install [Ollama](https://ollama.com/download), then in a separate terminal:

```sh
ollama serve              # starts the local server on http://localhost:11434
ollama pull llama3.1      # download the default model
```

## Run

```sh
cargo run -p pkdealer_agent_ollama -- --name llama
```

To use a different model:

```sh
cargo run -p pkdealer_agent_ollama -- --model mistral
# or
OLLAMA_MODEL=gemma2 cargo run -p pkdealer_agent_ollama
```

To point at a non-default host (e.g. Ollama running on another machine):

```sh
OLLAMA_HOST=http://192.168.1.10:11434 cargo run -p pkdealer_agent_ollama
```

## Environment variables

| Variable | Default | Purpose |
|----------|---------|---------|
| `OLLAMA_HOST` | `http://localhost:11434` | Ollama HTTP host |
| `OLLAMA_MODEL` | `llama3.1` | Model identifier override |
| `PKDEALER_ENDPOINT` | `http://127.0.0.1:50051` | gRPC service address |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4317` | OTel collector |
| `OTEL_SDK_DISABLED` | — | Set to `true` to skip OTel init |

## Architecture

`OllamaBackend` lives in `src/lib.rs` and implements
`pkdealer_agent_llm::LlmBackend`. The binary in `src/main.rs` parses args,
constructs an `OllamaBackend`, wraps it in an `LlmPokerAgent`, and hands it
to `pkdealer_agent_core::run_agent`. All poker-specific logic — prompt
construction, response parsing, fallback decisions — lives in
`pkdealer_agent_llm` and is shared with the Claude agent.
