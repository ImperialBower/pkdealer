#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.10"
# dependencies = []
# ///
"""Build PokerBench-guided Ollama models from the downloaded dataset (EPIC-43/44).

The models you run (``gemma2``, ``llama3.1``, ``mistral``) can't be weight-trained
on a 16GB Mac, so instead of fine-tuning we bake a curated set of PokerBench
*solver-optimal* decisions into a derived model's system prompt. ``ollama create``
produces ``pkpoker-gemma`` / ``pkpoker-llama`` / ``pkpoker-mistral`` that decide
with those examples in context — no GPU, no cloud, no cost.

Pipeline:
    make pokerbench-data      # download the dataset first
    make pokerbench-models    # this script: sample -> Modelfile -> ollama create

Then seat them via arena.toml entries ``pkgemma`` / ``pkllama`` / ``pkmistral``.

Data format (RZ412/PokerBench ``*_prompt_and_label.json``): a JSON array of
``{"instruction": <scenario text>, "output": "<action>"}`` objects.
"""

from __future__ import annotations

import argparse
import json
import random
import subprocess
import sys
from pathlib import Path

# Derived Ollama model name -> base model it builds FROM. The base must already
# be pulled (`ollama pull gemma2`). These names are what arena.toml seats and
# what AgentFidelity.model reports, so they also key PKDEALER_PRICE_AS.
DEFAULT_MODELS: dict[str, str] = {
    "pkpoker-gemma": "gemma2",
    "pkpoker-llama": "llama3.1",
    "pkpoker-mistral": "mistral",
    # Small/fast base (~3B) for low-latency play. The PokerBench data is baked
    # into the system prompt, not the weights, so it ports to any base with no
    # retraining — this one just runs far faster than the 9B gemma2 above.
    "pkpoker-qwen": "qwen2.5:3b",
}

# Test sets are smallest (1k / 10k) and ideal to sample from. The trainer-sized
# 60k/500k files are also accepted if present.
PROMPT_LABEL_FILES = [
    "preflop_1k_test_set_prompt_and_label.json",
    "postflop_10k_test_set_prompt_and_label.json",
    "preflop_60k_train_set_prompt_and_label.json",
    "postflop_500k_train_set_prompt_and_label.json",
]

SYSTEM_PREAMBLE = (
    "You are an expert no-limit Texas Hold'em poker player. Decide the optimal "
    "action for the situation you are given. Below are example decisions labelled "
    "with the solver-optimal action; use the same reasoning and answer in the same "
    "terse form (e.g. `fold`, `check`, `call`, `raise 3.0`, `bet 10.0`, `all-in`). "
    "Answer with the action only — no explanation."
)


def load_examples(data_dir: Path) -> list[dict[str, str]]:
    """Loads every available prompt+label record, grouped by street via filename."""
    records: list[dict[str, str]] = []
    found_any = False
    for fname in PROMPT_LABEL_FILES:
        path = data_dir / fname
        if not path.exists():
            continue
        found_any = True
        street = "preflop" if fname.startswith("preflop") else "postflop"
        try:
            data = json.loads(path.read_text())
        except json.JSONDecodeError as exc:
            print(f"warning: could not parse {fname}: {exc}", file=sys.stderr)
            continue
        for item in data:
            instruction = item.get("instruction")
            output = item.get("output")
            if isinstance(instruction, str) and isinstance(output, str):
                records.append(
                    {"street": street, "instruction": instruction.strip(), "output": output.strip()}
                )
    if not found_any:
        print(
            f"error: no PokerBench files found in {data_dir}.\n"
            "Run `make pokerbench-data` first (downloads ~720MB from HuggingFace).",
            file=sys.stderr,
        )
        sys.exit(1)
    return records


def sample_balanced(records: list[dict[str, str]], n: int, seed: int) -> list[dict[str, str]]:
    """Samples ``n`` examples split roughly evenly between preflop and postflop."""
    rng = random.Random(seed)
    preflop = [r for r in records if r["street"] == "preflop"]
    postflop = [r for r in records if r["street"] == "postflop"]
    half = n // 2
    chosen = rng.sample(preflop, min(half, len(preflop)))
    chosen += rng.sample(postflop, min(n - len(chosen), len(postflop)))
    # Top up from whatever remains if one street was short.
    if len(chosen) < n:
        remaining = [r for r in records if r not in chosen]
        chosen += rng.sample(remaining, min(n - len(chosen), len(remaining)))
    rng.shuffle(chosen)
    return chosen


def build_system_prompt(examples: list[dict[str, str]]) -> str:
    """Renders the preamble + numbered example decisions into a SYSTEM block."""
    lines = [SYSTEM_PREAMBLE, "", "Example decisions:"]
    for i, ex in enumerate(examples, 1):
        lines.append(f"\n[{i}] {ex['instruction']}")
        lines.append(f"Optimal action: {ex['output']}")
    return "\n".join(lines)


def write_modelfile(out_dir: Path, derived: str, base: str, system: str) -> Path:
    """Writes a Modelfile that derives ``derived`` FROM ``base`` with the system prompt."""
    out_dir.mkdir(parents=True, exist_ok=True)
    path = out_dir / f"Modelfile.{derived}"
    # Triple-quote the system value so newlines survive in the Modelfile.
    content = f'FROM {base}\n\nSYSTEM """\n{system}\n"""\n'
    path.write_text(content)
    return path


def ollama_create(derived: str, modelfile: Path) -> bool:
    """Runs `ollama create`; returns True on success."""
    print(f"  ollama create {derived} -f {modelfile}")
    result = subprocess.run(
        ["ollama", "create", derived, "-f", str(modelfile)],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        print(f"  FAILED: {result.stderr.strip()}", file=sys.stderr)
        return False
    return True


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data-dir", type=Path, default=Path("data/pokerbench"))
    parser.add_argument("--out-dir", type=Path, default=Path("build/pokerbench-models"))
    parser.add_argument("--examples", type=int, default=12, help="few-shot examples per model")
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="write Modelfiles only; do not run `ollama create`",
    )
    args = parser.parse_args()

    records = load_examples(args.data_dir)
    print(f"loaded {len(records)} PokerBench examples from {args.data_dir}")
    examples = sample_balanced(records, args.examples, args.seed)
    system = build_system_prompt(examples)
    print(f"sampled {len(examples)} examples → ~{len(system.split())} words of system prompt\n")

    failures = 0
    for derived, base in DEFAULT_MODELS.items():
        modelfile = write_modelfile(args.out_dir, derived, base, system)
        print(f"{derived}  (FROM {base})  → {modelfile}")
        if not args.dry_run and not ollama_create(derived, modelfile):
            failures += 1

    if args.dry_run:
        print("\ndry-run: Modelfiles written, no models created.")
    elif failures:
        print(f"\n{failures} model(s) failed — is `ollama serve` running and the base pulled?")
        return 1
    else:
        print("\nPokerBench-guided models ready. Seat them: ./bin/arena pkgemma gto lag")
    return 0


if __name__ == "__main__":
    sys.exit(main())
