# Confirmatory benchmark 3 — pre-registration

Frozen before the corpus is built and before any confirm3 A/B result is produced.
Date: 2026-10-23.

## Question

Does the frozen retrieval pipeline A outperform the naive lexical baseline B on
the primary hard-query stratum, under the same product threshold used for v2 and
holdout?

This is a confirmation run, not a tuning set. If it fails, the failure is
retained. No query, label, weight, threshold, corpus file, or implementation
change may be made in response before the result is archived.

## Corpus construction

- 30 public incidents that do not appear in v1, v2, holdout, or confirm2.
- Source list: `https://github.com/danluu/post-mortems` (retrieved 2026-10-23).
- Each incident contributes five stored cases: literal x2, paraphrase x2,
  semantic x1.
- Evaluation has 150 queries: literal 60, paraphrase 60, semantic 30.
- For literal rows, the evaluation query equals the stored case query. For
  paraphrase and semantic rows, the evaluation query is a distinct sibling
  wording of the same incident. The expected target is the stored case whose
  case-query is the closest wording, not a duplicate case.
- All five sibling cases for one incident share the same published root-cause
  summary. This deliberately tests whether a title/query field can distinguish
  the exact case among near-duplicate incident siblings.
- The exact original incident URL is stored in every case and in
  `eval-real/seed-confirm3.ps1`.
- Facts are taken from the public summaries. Chinese root-cause summaries and
  queries are translations or paraphrases; no incident, log, metric, or fix is
  invented.

Build script:

- `eval-real/seed-confirm3.ps1`
- SHA256 `E8A4C32B676A80BB46BB2B09774B4FE9C07260ACB5A4A1C0B6828770E45A4FE6`

## Labels and evidence level

- Target-only mechanical labels: the case derived from the incident has
  `rel=2`; other candidates are unlabelled and count as 0.
- `judge.by=mechanical-target-confirm3-v1` explicitly marks the labels as
  mechanical rather than human blind adjudication.
- This benchmark tests target-ranking generalization. It does not replace M4
  human relevance evidence.

## Frozen implementation

Binary: `rs/target/release/scx.exe`, SHA256
`DA7C8500B18147A0B7AD4769AA8A4D3A950AA3ACCF3938C859A1EECA15551B32`.

Source SHA256:

| file | SHA256 |
|---|---|
| `rs/src/memory/bucket.rs` | `5F9B0221BD1CDE1EC4FE3AE2A15ED04F684FDA045FB946669D28D1620390B81B` |
| `rs/src/memory/index.rs` | `012024FD7B6D5D31ED7348F83641DC4FF80624718597BDAA8187A4345BA7E8AC` |
| `rs/src/memory/edge.rs` | `3037570439811BA2266C38ED9FB9C1CF5416F1FB1476EF6497F859D002DBF9D7` |
| `rs/src/memory/meta.rs` | `9542B941A20BF32565736E3C598B35F961E6A822C92C5B04F431AF7E49A00E06` |
| `rs/src/memory/postings.rs` | `2BEF5E036B4D82143C080477DBC5C41A80D379434D99712E328FC4D49F1172B1` |
| `rs/src/memory/recall.rs` | `CBFEB743FD076853292FA715E2E2E37D85709DCE485593A7A14980CDF980A3F5` |

Base revision: `30a10e7ca0807f355948d83325c3ee05cfc980e0` plus the frozen
working-tree source hashes above.

Frozen defaults relevant to the test:

- `query_field_weight=1.0`
- `query_field_trigger_sim=0.30`
- query-field scoring affects ranking only; it is excluded from lexical `sim`
  and therefore cannot lower L1 confidence.
- low-confidence query-field reordering applies only to CJK queries; exact
  score ties can still use query-field evidence for any language.
- case buckets are multi-label; retrieval keeps a case if any candidate bucket
  intersects any of its labels.
- inverted-index records carry both case-query terms and multi-label buckets.

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
- The hard-layer actual n is reported. If it is below 55, the report states that
  the v2 power basis does not transfer automatically and labels the result
  underpowered even if the point estimate passes.

## Run protocol (once)

1. Record SHA256 of this file and the seed script.
2. Build the corpus and baseline; pre-seal the corpus.
3. Run A/B with:

   ```powershell
   .\rs\target\release\scx.exe --eval .\eval-real\confirm3-baseline.json `
     --gate --eval-verbose --mem-dir .\eval-real\confirm3-mem
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
  this is intentional and matches the v1/v2/holdout design, but it does not
  measure retrieval when only one case per incident exists.
