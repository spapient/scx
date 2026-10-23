# Confirmatory benchmark 2 — pre-registration

Frozen before the corpus is built and before any A/B result is produced.
Date: 2026-10-23.

## Question

Does the current complete retrieval pipeline A outperform the naive lexical
baseline B on the primary hard-query stratum, under the same product quality
threshold used for v2?

This benchmark tests the frozen implementation described below. It is not a
tuning set. If it fails, the failure is retained and no threshold, weight, query,
or label is changed in response.

## Corpus construction

- 30 public incidents, one stored case per incident.
- Source list: `https://github.com/danluu/post-mortems` (retrieved 2026-10-23).
- The exact original incident URL is embedded in each stored case and repeated
  in `eval-real/seed-confirm2.ps1`.
- No incident URL from v1, v2, or `holdout-*` is reused.
- Each stored case has one case-query field and one root-cause summary.
- Five evaluation queries target that same case: literal x2, paraphrase x2,
  semantic x1. Thus there are 30 documents and 150 evaluation cases.
- One case per incident is deliberate: a correct retrieval cannot be scored as
  a miss merely because a same-incident sibling case ranked first.
- The Chinese semantic queries and root-cause summaries are translations or
  paraphrases of the public incident summaries; no incident or log is invented.

Build scripts:

- `eval-real/seed-confirm2.ps1`
- `scripts/holdout-build.ps1` with
  `-JudgeBy mechanical-target-confirm2-v1`.

## Labels and evidence level

- Target-only mechanical labels: the one case derived from the incident has
  `rel=2`; no other case exists for that incident.
- `judge.by=mechanical-target-confirm2-v1` explicitly marks this as mechanical,
  not human blind adjudication.
- This benchmark establishes target-ranking generalization. It does not replace
  M4 human relevance evidence.

## Frozen implementation

Binary: `rs/target/release/scx.exe`, SHA256
`39A856A31FBF161C2B6F749E1FDC99F23542C180D711C253910487DF4FD35239`.

Source SHA256:

| file | SHA256 |
|---|---|
| `rs/src/memory/bucket.rs` | `5F9B0221BD1CDE1EC4FE3AE2A15ED04F684FDA045FB946669D28D1620390B81B` |
| `rs/src/memory/index.rs` | `012024FD7B6D5D31ED7348F83641DC4FF80624718597BDAA8187A4345BA7E8AC` |
| `rs/src/memory/edge.rs` | `3037570439811BA2266C38ED9FB9C1CF5416F1FB1476EF6497F859D002DBF9D7` |
| `rs/src/memory/meta.rs` | `9542B941A20BF32565736E3C598B35F961E6A822C92C5B04F431AF7E49A00E06` |
| `rs/src/memory/postings.rs` | `2BEF5E036B4D82143C080477DBC5C41A80D379434D99712E328FC4D49F1172B1` |
| `rs/src/memory/recall.rs` | `F99B8A7452525952DBC39A21CA00E427A7142D81674BF732CBF356DB9E1699F1` |
| `eval-real/seed-confirm2.ps1` | `051196C17455AA2B41B06DCCC8CAA2F01D3F57ECD9CCF598463DDDC7B0456058` |

Base revision: `30a10e7ca0807f355948d83325c3ee05cfc980e0` plus the frozen
working-tree source hashes above.

Frozen defaults relevant to the test:

- `query_field_weight=1.0`
- `query_field_trigger_sim=0.30`
- query-field scoring affects ranking only; it is excluded from lexical `sim`
  and therefore cannot lower L1 confidence.
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
   .\rs\target\release\scx.exe --eval .\eval-real\confirm2-baseline.json `
     --gate --eval-verbose --mem-dir .\eval-real\confirm2-mem
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
