# Confirmatory benchmark 4 — pre-registration

Frozen before the corpus is built and before any confirm4 A/B result is produced.
Date: 2026-10-23.

## Question

Does the frozen retrieval pipeline A outperform the naive lexical baseline B on
the primary hard-query stratum, under the same product threshold used for v2,
holdout, confirm2, and confirm3?

This is a confirmation run, not a tuning set. If it fails, the failure is
retained. No query, label, weight, threshold, corpus file, or implementation
change may be made in response before the result is archived.

## Corpus construction

- 30 public incidents that do not appear in v1, v2, holdout, confirm2, or
  confirm3.
- Source list: `https://github.com/danluu/post-mortems` (retrieved 2026-10-23).
- Each incident contributes five stored cases: literal x2, paraphrase x2,
  semantic x1.
- Evaluation has 150 queries: literal 60, paraphrase 60, semantic 30.
- For literal rows, the evaluation query equals the stored case query. For
  paraphrase and semantic rows, the evaluation query is a distinct sibling
  wording of the same incident.
- All five sibling cases for one incident share the same published root-cause
  summary. This tests whether query-field evidence can distinguish the exact
  case among near-duplicate incident siblings.
- The exact public source URL is stored in every case and in
  `eval-real/seed-confirm4.ps1`.
- Facts are taken from the public summaries. Chinese root-cause summaries and
  queries are translations or paraphrases; no incident, log, metric, or fix is
  invented.

Pre-build integrity audit:

- 30 incidents, 30 unique incident ids, 30 unique canonical source URLs.
- 150 evaluation queries, all 150 unique after case-folding and trimming.
- 240 stored query strings, all 240 unique after case-folding and trimming.
- Compared against `seed-real.ps1`, `seed-v2.ps1`, `seed-holdout.ps1`,
  `seed-confirm2.ps1`, and `seed-confirm3.ps1`: zero canonical URL overlaps and
  zero exact query/text overlaps.
- PowerShell parser reports zero syntax errors.
- Frozen Jaccard stratification estimate: hard n=80 (paraphrase 54, semantic
  26). This is above the 55-case power floor; actual n is recomputed after the
  run.

Build script:

- `eval-real/seed-confirm4.ps1`
- SHA256 `627DF635EB59652852691447C16587F7EDEBC238609DCC1123C2FDAF8EFF3F0E`

## Labels and evidence level

- Target-only mechanical labels: the case derived from the incident has
  `rel=2`; other candidates are unlabelled and count as 0.
- `judge.by=mechanical-target-confirm4-v1` explicitly marks the labels as
  mechanical rather than human blind adjudication.
- This benchmark tests target-ranking generalization. It does not replace M4
  human relevance evidence.

## Frozen implementation

Binary: `rs/target/release/scx.exe`, SHA256
`DD2C5C4491DE8A27554E82D056720DE5E9C4EEEFE088F3C9B11758EB13D773BE`.

Source and measurement manifest: `eval-real/confirm4-source.sha256`, 42 entries
(`rs/Cargo.toml`, `rs/Cargo.lock`, all `rs/src/**/*.rs`, the build/analysis
scripts). Manifest SHA256:
`9984738BD6EB834749710153A52173E7729CBAD83FB66B76D77C5E7CFF5C1228`.

Base revision: `30a10e7ca0807f355948d83325c3ee05cfc980e0` plus the frozen
working-tree manifest above.

Frozen defaults relevant to the test:

- `query_field_weight=1.0`
- `query_field_trigger_sim=0.30`
- query-field scoring affects ranking only; it is excluded from lexical `sim`
  and therefore cannot lower L1 confidence.
- low-confidence query-field reordering applies only to CJK queries; exact
  score ties can still use query-field evidence for any language.
- `synonyms_enable=1`, `synonym_weight=0.5`.
- BM25 `df/avgdl` statistics use the whole corpus; bucket filtering only
  narrows candidates.
- case buckets are multi-label; retrieval keeps a case if any candidate bucket
  intersects any of its labels.
- `mmr_enable=0`.

Pre-freeze verification evidence:

- `cargo build --release --locked`: exit 0.
- `cargo test --locked`: exit 0.
- `cargo clippy --all-targets --locked -- -D warnings`: exit 0.

## Pre-registered decision rule

- Primary stratum: `tier:hard` as computed by `eval-real/v2-strata.ps1`
  (token Jaccard of evaluation query vs target case-query less than 0.2).
- Primary metric: paired MRR@10 difference `Delta = MRR_A - MRR_B`.
- `MDE=0.05` (same product-decision threshold as v2).
- Advantage is established only if all of the following hold:
  1. `Delta >= 0.05`;
  2. paired bootstrap 95% CI lower bound `> 0`;
  3. permutation `p < 0.05` (10,000 resamples, fixed seed 20261023);
  4. `--gate` exits 0;
  5. literal layer A is not worse than B.
- Any other outcome is reported as "advantage not established". Secondary
  semantic/all results cannot overrule a failed hard stratum.
- If the actual hard-layer n is below 55, the v2 power basis does not transfer
  automatically and the result is labelled underpowered even if the point
  estimate passes.

## Run protocol (once)

1. Record SHA256 of this file, the seed script, and the source manifest.
2. Build the corpus and baseline; pre-seal the corpus.
3. Run A/B with:

   ```powershell
   .\rs\target\release\scx.exe --eval .\eval-real\confirm4-baseline.json `
     --gate --eval-verbose --mem-dir .\eval-real\confirm4-mem
   ```

4. Derive rank-diff and strata from that single verbose run.
5. Run `eval-real/v2-significance.ps1` with 10,000 bootstrap and permutation
   samples.
6. Archive gate output, significance CSV, rank-diff CSV, strata CSV, and verbose
   output. Do not rerun after changing any frozen input.

## Known limits

- Mechanical target labels only; no second human adjudicator in this benchmark.
- 30 incidents are a small sample; the hard stratum may be underpowered.
- The corpus is selected from a public incident index, not randomly sampled
  from all production outages.
- A/B baseline B is lexical; the result is not evidence against embedding-based
  retrieval systems.
- Five sibling cases per incident make the target a near-duplicate ranking task;
  this is intentional and matches the v1/v2/holdout/confirm3 design, but it does
  not measure retrieval when only one case per incident exists.
