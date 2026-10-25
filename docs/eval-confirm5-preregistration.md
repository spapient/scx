# Confirmatory benchmark 5 — pre-registration

Frozen before the corpus is built and before any confirm5 A/B result is produced.
Date: 2026-10-23.

> **Refactor note (2026-10-25).** The decision rule and corpus protocol in this
> frozen preregistration are unchanged; only artifact paths/commands were
> updated. Historical `eval-real/` files remain recoverable from git commit
> `5ada90d`. Current equivalents: corpus `eval/corpora/confirm5.json`, seed command
> `python3 eval/scx_eval.py seed ...`, baseline
> `python3 eval/scx_eval.py baseline ...`, and statistics
> `python3 eval/scx_eval.py stats ...`. The frozen input hashes below are
> historical; current hashes are in `docs/evaluation-results.md`.

## Question

Does the frozen retrieval pipeline A outperform the naive lexical baseline B on
the primary hard-query stratum, under the same product threshold used for v2,
holdout, confirm2, confirm3, and confirm4?

This is a confirmation run. If it fails, the failure is retained. No query,
label, weight, threshold, corpus file, or implementation change may be made in
response before the result is archived.

## Provenance and prior runs

- confirm4 failed its first clean run (`exit=2`): paraphrase MRR 0.287 vs 0.288
  and semantic r@10 0.967 vs 1.000.
- Failure analysis found two low-confidence queries where a query-side bucket
  (`disk` / `runtime`) hard-filtered the correct `db` case before ranking.
- The implementation was changed to bypass bucket filtering when first-pass
  lexical confidence is below 0.30. confirm4 then passed as a diagnostic, but
  confirm4 is no longer a confirmation set because the fix was motivated by its
  observed failures.
- confirm5 uses 30 different incidents and was constructed after the fix,
  without running any confirm5 A/B or inspecting any confirm5 ranking result.

## Corpus construction

- 30 public incidents that do not appear in v1, v2, holdout, confirm2, confirm3,
  or confirm4.
- Source list: `https://github.com/danluu/post-mortems` (retrieved 2026-10-23).
- Each incident contributes five stored cases: literal x2, paraphrase x2,
  semantic x1.
- Evaluation has 150 queries: literal 60, paraphrase 60, semantic 30.
- Literal evaluation queries equal the stored case query. Paraphrase and
  semantic evaluation queries are distinct sibling wordings of the same
  incident.
- All five sibling cases for one incident share the published root-cause
  summary, making the exact-case decision depend on query-field evidence rather
  than only on the incident-level body.
- The public source URL is stored in every case; facts are translations or
  paraphrases of the public summaries, not invented incidents.

Pre-build integrity audit:

- 30 incidents, 30 unique ids, 30 unique exact source URLs.
- 150 evaluation queries, all unique after case-folding and trimming.
- 240 stored query strings, all unique after case-folding and trimming.
- Compared against `seed-real.ps1` (v1), `seed-v2.ps1`, `seed-holdout.ps1`,
  `seed-confirm2.ps1`, `seed-confirm3.ps1`, and `seed-confirm4.ps1`: zero
  canonical URL overlaps and zero exact query/text overlaps.
- PowerShell parser reports zero syntax errors in the seed script.
- Frozen Jaccard stratification estimate: hard n=69 (paraphrase 54, semantic
  15), above the 55-case power floor. Actual n is recomputed after the run.

Corpus files:

- `eval/corpora/confirm5.json` (current normalized corpus; historical incident JSON
  SHA256 `E12FDD3C62E6E5C4B192BE423DC6F3070097DE033D41D0426E95E6E543765704`)
- Seed via `python3 eval/scx_eval.py seed eval/corpora/confirm5.json ...`
  (historical `seed-confirm5.ps1` SHA256
  `AA7D90BDA0251E8E1D34AE8E7B8834310481785690E3297B64F16DE830792BDC`)

## Labels and evidence level

- Target-only mechanical labels: the derived case has `rel=2`; other candidates
  are unlabelled and count as 0.
- `judge.by=mechanical-target-confirm5-v1` marks the labels as mechanical rather
  than human blind adjudication.
- This benchmark tests target-ranking generalization. It does not replace M4
  human relevance evidence.

## Frozen implementation

Binary: `target/release/scx.exe`, SHA256
`0469182BD8FED2A32F5288D188728ECCC8852129D7124C38643C4DE18F7D6FCD`.

Source and measurement manifest (historical): `confirm5-source.sha256`, 43 entries
(`Cargo.toml`, `Cargo.lock`, all `src/**/*.rs`, seed data, and the
build/analysis scripts). Manifest SHA256:
`9856A9CFE3D69D437A446028DFFF50EE77A41F1E8D0F2845826BD0E279FCCCD7`.

Base revision: `30a10e7ca0807f355948d83325c3ee05cfc980e0` plus the frozen
working-tree manifest above.

Frozen defaults relevant to the test:

- `query_field_weight=1.0`
- `query_field_trigger_sim=0.30`
- query-field scoring affects ranking only and is excluded from lexical `sim`.
- low-confidence query-field reordering applies only to CJK queries; exact
  score ties can still use query-field evidence for any language.
- `bucket_bypass_sim=0.30`: if first-pass top `sim` is below this value, the
  query-side bucket filter is reverted to full lexical candidate scoring for all
  remaining passes.
- `synonyms_enable=1`, `synonym_weight=0.5`.
- BM25 `df/avgdl` statistics use the whole corpus; bucket filtering only narrows
  candidates.
- case buckets are multi-label; `mmr_enable=0`.

Pre-freeze verification evidence:

- `cargo fmt`: exit 0.
- `cargo clippy --all-targets --locked -- -D warnings`: exit 0.
- `cargo test --locked`: exit 0 (121 library + 8 CLI + 5 integration + golden).
- `cargo build --release --locked`: exit 0.

## Pre-registered decision rule

- Primary stratum: `tier:hard` as computed by `eval/scx_eval.py stats`
  (token Jaccard of evaluation query vs target case-query less than 0.2).
- Primary metric: paired MRR@10 difference `Delta = MRR_A - MRR_B`.
- `MDE=0.05`.
- Advantage is established only if all of the following hold:
  1. `Delta >= 0.05`;
  2. paired bootstrap 95% CI lower bound `> 0`;
  3. permutation `p < 0.05` (10,000 resamples, fixed seed 20261023);
  4. `--gate` exits 0;
  5. literal layer A is not worse than B.
- Any other outcome is reported as "advantage not established". Secondary
  semantic/all results cannot overrule a failed hard stratum.
- If actual hard-layer n is below 55, the result is labelled underpowered even
  if the point estimate passes.

## Run protocol (once)

1. Record SHA256 of this file, the seed script, incidents JSON, and manifest.
2. Build the corpus and baseline; pre-seal the corpus.
3. Run A/B with:

   ```bash
   python3 eval/scx_eval.py seed eval/corpora/confirm5.json \
     --mem-dir /tmp/scx-confirm5 --csv /tmp/scx-confirm5.csv --force
   python3 eval/scx_eval.py baseline --csv /tmp/scx-confirm5.csv \
     --out /tmp/scx-confirm5-mechanical.json \
     --judge-by mechanical-target-confirm5-v1 \
     --judge-date 2026-10-23T00:00:00Z
   ./bin/run --eval /tmp/scx-confirm5-mechanical.json --gate --eval-verbose \
     --mem-dir /tmp/scx-confirm5
   ```

4. Derive rank-diff and strata from that single verbose run.
5. Run `python3 eval/scx_eval.py stats` with 10,000 bootstrap and permutation
   samples and `--cluster-size 5`.
6. Archive gate output, significance CSV, rank-diff CSV, strata CSV, and verbose
   output. Do not rerun after changing any frozen input.

## Known limits

- Mechanical target labels only; no human blind adjudication in this benchmark.
- 30 incidents are a small sample; the hard stratum may be underpowered.
- The corpus is selected from a public incident index, not randomly sampled.
- A/B baseline B is lexical; this is not evidence against embedding retrieval.
- Five sibling cases per incident make the target a near-duplicate ranking task;
  the result does not measure retrieval when only one case per incident exists.
