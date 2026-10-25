# scx evaluation kit

One tool, three corpora, and the frozen labels needed to reproduce the published
claims. Generated memory directories, verbose logs, rank-diff CSVs, and
significance CSVs are **outputs**, not tracked inputs.

Run `./bin/scx-eval` from the repository root on macOS/Linux, or
`python bin/scx-eval` on Windows. The evaluator uses only the Python standard
library; retrieval and gates remain the single Rust implementation under `src/`.

## Layout

| Path | Role |
|---|---|
| `../bin/scx-eval` | corpus seeding, baseline construction, blind review, paired statistics, packet verification |
| `confirm5.corpus.json` | confirm5 corpus: 30 public incidents, 150 stored cases |
| `v2.corpus.json` | v2 discriminative corpus: 30 public incidents, 150 stored cases |
| `real.corpus.json` | real-incident corpus: 40 cases from 11 public postmortems |
| `confirm5-policy.labels.json` | policy-materialized weak/negative control set (not human) |
| `confirm5-ai.labels.json` | 384/384 AI per-candidate labels (`per-item-ai-v1`) |
| `confirm5-hao.labels.json` | 384/384 human per-candidate labels (`per-item-human-v1`) |
| `v2.labels.json` | v2 final human-adopted labels (`judge.by=fei+hao`) |
| `real.labels.json` | real-incident case-level labels (`judge.by=fei+hao`) |
| `confirm5-40.packet.tsv` | frozen blind packet, 40 cases / 384 candidates |
| `confirm5-40-ai.tsv` | AI input packet retained for provenance |
| `confirm5-40-hao.tsv` | human input packet after the literal invariant correction |

The corpus JSON schema is deliberately benchmark-neutral:

```json
{
  "name": "example",
  "expected_incidents": 1,
  "expected_cases": 1,
  "incidents": [{
    "id": "incident-id",
    "source": "https://example.invalid/postmortem",
    "root_cause": "one-line public root cause",
    "fix": "one-line public remediation",
    "cases": [{
      "store_query": "query used to store the case",
      "text_query": "optional first line of the stored text",
      "eval_query": "query used by --eval",
      "layer": "literal"
    }]
  }]
}
```

`text_query` defaults to `store_query`. The seed command writes the same text
shape as the historical PowerShell seeds:

```text
<text_query>
公开复盘, 来源: <source>
根因: <root_cause>
```

Verified on 2026-10-25: seeding the three normalized corpora reproduces the
frozen case hash suffixes exactly (confirm5 150/150, v2 150/150, real 40/40),
so frozen baselines still match after the date prefix changes.

## Labeling contract

An evaluation set has 30–50 real cases, with at least 10 cases in each of
`literal`, `paraphrase`, and `semantic`. A case entry has:

| Field | Meaning |
|---|---|
| `query` | Retrieval query used by `--eval` |
| `layer` | `literal`, `paraphrase`, or `semantic` |
| `expected` | One or more real case ids (full id or hash suffix) |
| `rel` | Candidate id → `2` direct answer, `1` related, `0` unrelated; omitted expected ids default to 1 |
| `judge` | `{by,date,method}`; `null` means not yet adjudicated |
| `note` | Optional blind-review hint; no A/B scores |
| `judge_items` | Per-candidate provenance required by `--gate-human` |

`--gate` is a mechanical qualification gate: sample size, layer balance,
non-mechanical judge metadata, A not worse than B, and absolute floors.
`--gate-human` additionally requires complete per-candidate human provenance,
weak/negative controls, and NDCG A ≥ B. `--gate-items` permits non-human
per-candidate adjudication but is labeled as such and never satisfies the
human gate.

Use `blind` for candidate-level labels; use the table-to-CSV path only for
mechanical target labels. Never treat automatic annotations, policy-derived
controls, or a single annotator as independent human evidence.

## Reproduce confirm5

```bash
./bin/scx-eval seed eval/confirm5.corpus.json \
  --mem-dir /tmp/scx-confirm5 --csv /tmp/scx-confirm5.csv --force
./bin/scx-eval baseline --csv /tmp/scx-confirm5.csv \
  --out /tmp/scx-confirm5-mechanical.json \
  --judge-by mechanical-target-confirm5-v1 \
  --judge-date 2026-10-23T00:00:00Z
./bin/run --eval /tmp/scx-confirm5-mechanical.json --gate --eval-verbose \
  --mem-dir /tmp/scx-confirm5
```

The human and policy gates consume the frozen labels against the same rebuilt
memory:

```bash
./bin/run --eval eval/confirm5-hao.labels.json --gate-human --eval-verbose \
  --mem-dir /tmp/scx-confirm5
./bin/run --eval eval/confirm5-ai.labels.json --gate-items --mem-dir /tmp/scx-confirm5
./bin/run --eval eval/confirm5-policy.labels.json --gate-human --mem-dir /tmp/scx-confirm5
```

Expected outcomes: human `PASS / exit 0`; AI `--gate-items PASS` but
`--gate-human FAIL / exit 2`; policy `FAIL / exit 2`.

## Paired statistics

`stats` reads the deterministic verbose output and computes the paired MRR or
NDCG delta, bootstrap CI, and sign-flip permutation p-value. Difficulty tiers
are computed from the memory case query when `--mem-dir` is supplied; a
`case_index,tier` CSV can be passed instead.

```bash
./bin/run --eval eval/confirm5-hao.labels.json --gate-human --eval-verbose \
  --mem-dir /tmp/scx-confirm5 > /tmp/scx-confirm5-hao.txt
./bin/scx-eval stats \
  --baseline eval/confirm5-hao.labels.json \
  --verbose /tmp/scx-confirm5-hao.txt \
  --mem-dir /tmp/scx-confirm5 \
  --cluster-size 5 \
  --out /tmp/scx-confirm5-hao-stats.csv
```

`--cluster-size 5` adds the incident-cluster sensitivity row for the five-sibling
corpora. It is a sensitivity check, not a replacement for the pre-registered
case-level primary analysis.

## Blind review

```bash
./bin/scx-eval blind make \
  --baseline eval/confirm5-policy.labels.json --mem-dir /tmp/scx-confirm5 \
  --out /tmp/packet.tsv --manifest /tmp/manifest.json
./bin/scx-eval blind progress --packet /tmp/packet.tsv
./bin/scx-eval blind import \
  --baseline eval/confirm5-policy.labels.json --packet /tmp/packet.tsv \
  --out /tmp/human.json --judge <name> --date 2026-10-25T00:00:00Z
./bin/scx-eval blind check --baseline /tmp/human.json
./bin/scx-eval blind agree \
  --a /tmp/first-reviewer.json --b /tmp/second-reviewer.json
```

The `make` command is deterministic. Rebuilding the frozen confirm5 packet from
`confirm5-policy.labels.json` and a freshly seeded memory reproduces SHA256
`b8eebf2fcb78d0fb745c9760b55dd34d832a9bb976b377f3dfb8c7cea264093f`.
`agree` reports common per-candidate grades, raw agreement, Cohen's kappa, and
the disagreement list; use it for the two-reviewer arbitration loop.
Frozen label files may retain historical packet path strings; resolve the
packet by its recorded `judge.packet_sha256`, not by that string.

## Reproduce v2 and real

```bash
./bin/scx-eval seed eval/v2.corpus.json \
  --mem-dir /tmp/scx-v2 --csv /tmp/scx-v2.csv --force
./bin/run --eval eval/v2.labels.json --gate --mem-dir /tmp/scx-v2

./bin/scx-eval seed eval/real.corpus.json \
  --mem-dir /tmp/scx-real --csv /tmp/scx-real.csv --force
./bin/run --eval eval/real.labels.json --gate-human --mem-dir /tmp/scx-real
```

v2 is a development benchmark; its labels were AI-prefilled and then adopted by
two reviewers, so it is not an independent blind holdout. `real.labels.json` has only
case-level signatures, so `--gate-human` rejects it at the per-item provenance
check. Those failures are intentional evidence boundaries.

## Artifact policy

- Keep corpora, frozen labels, packets, and preregistrations.
- Do not commit memory directories, verbose logs, gate logs, rank-diff CSVs,
  strata CSVs, significance CSVs, or draft packets. Any of them can be rebuilt
  from the retained inputs with `../bin/scx-eval`.
- A result is quoted only with its corpus, label set, binary/config, gate mode,
  sample size, and statistics command.
- `eval-real/` was removed in the 2026-10-25 refactor. Historical intermediate
  artifacts remain recoverable from git commit `5ada90d`.
