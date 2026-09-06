**Native mini encoder validation — 2026-09-06**

The native Linux encoder is feasible: it uses the actual pinned mini model, runs in the server process without WASM/Node, and passes the published reference tokenizer/vector fixtures. This establishes native feasibility, not completion of the full four-platform and relevance release gates. **Latest measured holdout:** native NDCG@10 `0.82243`, versus lexical/concept `0.59539` and previous WASM hybrid `0.86649`; unrelated false recall `0/10`. The new implementation has a clear graded-relevance gain over text, but Chinese non-regression against the previous hybrid remains unproven (`0.80043` versus `0.86463`). Windows/macOS/ARM64 execution also remains unverified.

The source is MIT `soycaporal/ternlight` tag `v0.1.1`, revision `c6d2c0a35d14c574ed2898b3dbf95977bca07208`. `crates/search-encoder` preserves the five inference/tokenizer/parser modules and adapts the public entry point, embedded asset paths, error boundary, compile-time feature selection and concept expansion. The upstream MIT license and copyright accompany both code and weights.

The raw weights are extracted reproducibly from the static memory of published MIT `@ternlight/mini@0.1.1`; the source repository does not commit `model.bin`. The extraction tool verifies the TERN v1/int4 header, exact 4,839,512-byte model extent, trailing SHA256 over the body, and pinned whole-file digest. No reference WASM file is shipped. The tokenizer is the fixed upstream tokenizer JSON. Provenance and the published reference WASM hash are in `assets/search/models/mini/provenance.json`.

| Asset | Bytes | SHA256 |
|---|---:|---|
| Model | 4,839,512 | `07d8cfdba5773ad69a3fe6164b6c964e87b2368cc3ad6c2bdaf8566f2e5b6c98` |
| Tokenizer | 711,396 | `d241a60d5e8f04cc1b2b3e9ef7a4921b27bf526d9f6050ab90f9267a1f9e5c66` |
| Public vectors | 946,176 | See the generated manifest |

The corpus contains 308 sites from source revision `dcd825aa1684680a52e6a85b42fb410b5551e156`, two normalized 384-dimensional float32 little-endian vectors per site. Its reviewed whitelist includes names, official aliases, exact verified hosts, PTD identity pairs, public resource metadata, concise descriptions, sources and phonetics. A parsing issue with brace-wrapped Rust hostname match arms was caught by the core tests and corrected: the import now produces 300 explicit PTD pairs, including `mteam` → `m-team`. A follow-up identity audit requires an explicit PTD definition source citation or exact canonical hostname evidence for every pair, rather than trusting identical ID spelling; it additionally recovers six verified namespace differences. No fuzzy ID conversion is used.

The manifest verifies every production input hash, row order, shape, endian format, model/tokenizer revision, template and normalization. Runtime also validates actual embedded model/tokenizer digests once before initialization. Input corruption and semantic compatibility failures are returned as errors so the search layer can expose its degradation state.

**Reference and asset checks**

`cargo test -p search-encoder --release` passes three integration tests covering pinned model/tokenizer digests, shared concept expansion and 20 reference inputs. The parity fixtures cover English, Chinese, mixed scripts, fullwidth letters, diacritics, punctuation, empty/blank strings and truncation beyond the 128-token budget. Token IDs must be identical; vector tolerance is maximum absolute difference ≤ `2e-5` with unit norm. The initial five-input Linux probe produced exactly identical floats (maximum difference zero) against the published WASM. The 20-case test passes the declared cross-platform tolerance.

`cargo run -p search-encoder --release --example assets -- verify` validates 308 rows and all 616 vectors: complete hashes, length, row mapping, finite float values and squared norms in `[0.998, 1.002]`. A second native build produced byte-for-byte identical catalog, vector and manifest files in the measured Linux environment. Maintainers can repeat import/build/verify without modifying the adjacent source checkout; normal builds use only committed assets. Commands are in `crates/search-encoder/README.md`.

**Measured cost**

The measurement host is Linux x86_64, Intel N150 (four physical cores, no GPU), Rust `1.97.1 (8bab26f4f 2026-07-14)`, release opt-level 3. The standalone benchmark process was restricted with `os.sched_setaffinity(0, {0,1})` to two cores. It performs 100 embeddings across five Chinese/English expanded domain queries. Other agents were compiling concurrently; these are observed local measurements rather than a dedicated-device service benchmark.

| Measurement | Result |
|---|---:|
| Model/tokenizer initialization | 17.72 ms |
| First short embedding after init | 7.07 ms |
| Warm p50 / p95 | 10.39 / 12.32 ms |
| Warm maximum | 13.11 ms |
| Entire standalone process peak RSS | 20,312 KiB |
| Stripped standalone probe | 9,497,360 bytes |
| Gzip standalone probe | 5,733,250 bytes |

The final measurement includes the full-file pinned model/tokenizer hash checks and model internal checksum verification. RSS is peak for the whole probe, not incremental resident memory of kirara. The probe includes its own tokenizer/runtime code; its file size is not the incremental server size. The complete Git asset directory is 7,532,931 bytes raw, including metadata, all evaluation histories and licenses; the sum of separately gzip-compressed files is 5,263,859 bytes. Production assets plus their notices/provenance total 7,218,270 bytes before compression. The root integration report measures the actual same-configuration server release delta and relative growth. No first-run files are created.

**Platform status**

| Target | Status |
|---|---|
| Linux x86_64 GNU | Native release build, execution, reference tests and benchmark pass |
| Windows x86_64 GNU | `cargo check -p search-encoder --target x86_64-pc-windows-gnu` passes; no Windows execution |
| macOS ARM64 | `cargo check -p search-encoder --target aarch64-apple-darwin` passes; no macOS execution |
| Linux x86_64 musl | Release test executable links and runs: 3 tests pass, including all 20 reference cases |
| Linux ARM64 musl | `cargo check -p search-encoder --target aarch64-unknown-linux-musl` passes; no ARM64 execution |

A successful cross-target `cargo check` is type checking, not successful platform linking or execution. These results cannot certify the required four-platform runtime matrix.

**Domain quality and limits**

The BERT uncased tokenizer is not a Chinese semantic model. Chinese original text is preserved and recognized domain concepts are prepended in English using exactly the same expansion for query and corpus. Names and aliases use deterministic search outside the model.

The initial uncalibrated domain probe returned the following semantic-only top five, without pretending the scores are probabilities:

| Query | Raw top five catalog IDs |
|---|---|
| 无损音乐 | dicmusic, kimoji, opencd, orpheus, redacted |
| 适合新手的动漫站 | animelovers, nekobt, bakabt, snowpt, animez |
| rare arthouse cinema | cinemaz, sdbits, anthelion, awesomehd, beitai |
| 足球赛车直播 | sportscult, f1carreras, ptfans, springsunday, retroflix |

The irrelevant Chinese query `天气预报明天下雨` reached raw cosine `0.60349`, and random Latin `xqzvjk qprst nonsense` reached `0.48794`. This rejects `0.58` as a sufficient standalone cutoff. The core was given a provisional absolute cutoff `0.65` plus conservative semantic eligibility. This is not universal calibration: out-of-domain Chinese, ambiguous terms and unsupported synonyms can still miss or mis-rank. A model score does not establish beginner friendliness or live signup availability.

`assets/search/evaluation.json` contains 30 additional held-out queries: 20 descriptive Chinese/English queries across ten resource domains and ten unrelated requests. Explicit graded judgments use public categories/content types/specialties (specialist examples grade 2, broader matching catalogs grade 1). This is a transparent domain relevance proxy, not independent user research or exhaustive manual labeling. Query wording was not used to select the provisional cutoff. The comparison uses the same judgments for the upstream pt-sites lexical/concept baseline, its existing WASM hybrid, and kirara's full native hybrid helper. The first complete measurement failed the semantic quality gate:

| Channel | NDCG@10 (20 descriptive) | Precision@5 | Unrelated requests returning results |
|---|---:|---:|---:|
| Existing pt-sites lexical/concept | 0.45150 | 0.69000 | 3 / 10 |
| Existing pt-sites WASM hybrid | 0.74389 | 0.88000 | 10 / 10 |
| New native hybrid, first evaluation | 0.38534 | 0.43000 | 0 / 10 |

The existing UI retains all semantic neighbors, so its 10/10 unrelated recall reflects that baseline policy rather than a tuned absolute threshold. The new path rejected all ten English descriptive queries, including mapped domain words surrounded by unrecognized prose. Nine of ten Chinese descriptive queries returned domain candidates. This shows the native model itself works, but the conservative per-word query classifier was preventing complete descriptive spans from reaching ranking. Exact model parity does not compensate for this end-to-end regression. The observed new hybrid is worse than both baselines on this fixture and cannot be called accepted.

Machine-readable per-query ranks and metrics are preserved in `assets/search/evaluation-results.json`. Once these results inform changes, this fixture is a diagnostic/regression set, not a fresh independent holdout; a new holdout must assess the revised parser. The query/parser work belongs to the search core and can change independently of the pinned model/vector assets.

Reproduce the comparison:

```sh
python3 -c 'import json; print(json.dumps([r["query"] for r in json.load(open("assets/search/evaluation.json"))["queries"]]))' > /tmp/search-queries.json
cargo run --release --example search-eval < /tmp/search-queries.json > /tmp/search-results.json
node crates/search-encoder/tools/evaluate.mjs ../pt-sites /tmp/search-results.json
```


**Second holdout after English span routing**

The core then routed unquoted English prose of at least three words as a semantic span while retaining explicit fields, quotes, names and negation. The absolute threshold remained `0.65`. The first fixture, now diagnostic, improved from NDCG@10 `0.38534` to `0.57878`, compared with the unchanged WASM baseline `0.74389`.

A second fixture of 30 fresh queries was kept in `/tmp` and not shown to the core agent until after the parser was frozen and the evaluation completed. It contains ten English natural sentences, ten Chinese descriptions and ten unrelated requests; no easy exact-name queries. The fixture and results are now preserved as `evaluation-heldout-v2.json` and `evaluation-heldout-v2-results.json`.

| Channel | NDCG@10 | Precision@5 | Unrelated false recall |
|---|---:|---:|---:|
| Existing lexical/concept | 0.55440 | 0.86000 | 1 / 10 |
| Existing WASM hybrid | 0.83387 | 0.97000 | 10 / 10 |
| Native hybrid after span routing | 0.51812 | 0.63000 | 0 / 10 |

By language, native/old-WASM NDCG@10 was `0.79644 / 0.85330` for Chinese and `0.23981 / 0.81444` for English. This run also fails the quality gate. Inspection after measurement found sentence-final punctuation was excluded from the parser's alphabetic prose condition, leaving words such as `quality?` and `events.` as hard literal constraints. The pronoun `me.` also collided with the official MilkIE alias `ME` inside prose. These are query-classification defects; fixing them requires another independent holdout rather than relabeling this result as a pass.

The isolated measurement helper imported the exact frozen repository search module with the repository Cargo.lock dependency versions; it excluded unrelated server/API modules. The checked-in `examples/search-eval.rs` runs the same search code through the normal workspace build. This distinction affects build cost, not the ranking algorithm or judgments.

A vector-only diagnostic (bypassing the service classifier, still using cosine `0.65`) on that second fixture gives NDCG@10 `0.62344`, Precision@5 `0.72`, and five unrelated false recalls. All five are unmapped Chinese inputs; the five unrelated English inputs remain below threshold. Raw cosine reaches `0.97073` for a cooking question and `0.92801` for an airport-bus question. This confirms that unmapped Chinese can collapse to highly similar BERT token representations: the model's raw score cannot certify domain relevance, and the conservative Chinese concept gate must remain. The example `semantic_rank` and `evaluation-semantic-only-results.json` preserve this diagnostic; these are not service results.

**Third independent holdout on the frozen parser snapshot**

The core fixed punctuation-aware English spans, ordinary pronouns that collided with aliases, and descriptive ranking so broad concept coverage does not automatically outrank stronger semantic evidence. Explicit fields, quoted literals, exact names, numeric/domain identities, negatives and collection-label hard filters remain protected. The threshold stayed at `0.65`, and unmapped Chinese still does not invoke unrestricted semantic retrieval.

Only after the parser/ranking freeze was declared was a third independent set constructed: 20 varied English/Chinese resource descriptions (including natural sentences and contractions) and ten unrelated requests. None is an exact-name easy case. The query contents were not shared with the core until the run completed. The data and full per-query top five/metrics are in `evaluation-heldout-v3.json` and `evaluation-heldout-v3-results.json`; the result also records the evaluated search-source and production-manifest hashes.

| Final channel | NDCG@10 | Precision@5 | Unrelated false recall |
|---|---:|---:|---:|
| Existing lexical/concept | 0.59539 | 0.93000 | 3 / 10 |
| Existing WASM hybrid | 0.86649 | 0.98000 | 10 / 10 |
| New native hybrid | 0.82243 | 0.92000 | 0 / 10 |

| Language (10 descriptive queries each) | Lexical/concept NDCG@10 | Previous WASM NDCG@10 | Native NDCG@10 |
|---|---:|---:|---:|
| Chinese | 0.56231 | 0.86463 | 0.80043 |
| English | 0.62847 | 0.86835 | 0.84443 |

The same final code also improves the second, now diagnostic set to NDCG@10 `0.79552` and Precision@5 `0.92`, with `0/10` unrelated false recall. Its earlier failed result is preserved instead of overwritten.

There is a clear NDCG gain over the text/concept baseline on the fresh third set (`+0.22704`), and all fresh unrelated requests are rejected. Precision@5 is slightly below the text baseline and both graded metrics remain below the old hybrid. In particular, the design's strict Chinese non-regression gate is **not established** by these results. The small source-derived judgment set is not a statistical non-inferiority study, and the hand-listed specialist grades are incomplete domain proxies. No further ranking tuning was performed on the third set. The final status is an implemented, functioning native hybrid with an honestly measured remaining relevance gap, not full release-gate acceptance.

Examples from that final holdout:

| Query | Native first five |
|---|---|
| I'm searching for high-fidelity music albums to collect. | dicmusic, kimoji, opencd, orpheus, redacted |
| 希望收集日本动漫和儿童动画作品 | animelovers, nekobt, bakabt, snowpt, animez |
| 希望找技术学习资料以及相关课程教程 | thegeeks, whupt, thevault, gfxpeers, xingtan |

For another full comparison, the evaluator accepts an optional fourth argument pointing to a fixture, for example:

```sh
node crates/search-encoder/tools/evaluate.mjs ../pt-sites /tmp/search-results.json assets/search/evaluation-heldout-v3.json
```

All three query sets are public synthetic evaluation data, not production/user query logs. Their diagnostic histories do not enter the server or frontend binary; production includes only the catalog, concepts, manifest, static vectors and pinned model/tokenizer. Production assets remained frozen throughout the final parser evaluations.

The reference and native ranking pipelines are deliberately not identical. The previous identity vector includes names, circle, categories, collection labels, specialties and groups; its description vector also includes language/service type. The native resource vector uses categories/content types, and its feature vector uses specialties/labels/descriptions/beginner text. Names stay in deterministic matching. The old hybrid also combines weighted lexical/vector scores, whereas the new search preserves explicit matching tiers. Exact tokenizer and encoder arithmetic parity therefore does not imply ranking parity. The third-run source hash identifies the evaluated snapshot; later general correctness fixes require a regression rerun before quoting these as current-code metrics, and must not be presented as a fresh independent holdout.

**Final current-code regression**

After the original independent third run, the core corrected the shared multiword-concept expansion invariant and added bounded spaced-pinyin/name handling. Following its explicit final freeze, the existing third fixture was run once more as a **regression, not a new holdout**. Source hashes were checked before and after the run to reject concurrent edits. The final numbers are unchanged: native NDCG@10 `0.8224324743`, Precision@5 `0.92`, unrelated false recall `0/10`; previous hybrid `0.8664929546 / 0.98 / 10/10`; lexical/concept `0.5953878605 / 0.93 / 3/10`.

`assets/search/evaluation-final-regression-results.json` preserves the final current-code output and complete source hash map. The original independent `evaluation-heldout-v3-results.json` remains untouched. No model, vector, template, threshold or relevance-label changes were made for this rerun.

| Evaluated source | SHA256 |
|---|---|
| `src/search/mod.rs` | `a910bbd7abe85e9c6e3c79447b04380060951a18443837591459fc1dd22c0505` |
| `src/search/semantic.rs` | `6da7cba2714fecd4a3680006bf89a65432667280534cd16398cf5829666a5bb3` |
| `crates/search-encoder/src/lib.rs` | `7f6b8c9a50608a65524a8e631ad26ec18853ab9589a4c8d2d2357f487c6cde5c` |
| `assets/search/manifest.json` | `6418bfa8b62e49d2df1bbe3952d37e8149719a166f6749cc3e9cc5bdcc0db8ed` |

**Actual Linux musl application release comparison**

The complete baseline and current application were subsequently built and executed for the shipping target `x86_64-unknown-linux-musl`, using the same Rust 1.97.1 release profile, `--locked --release --package kirara`, and explicit `musl-gcc` C compiler/linker. Debian musl-tools 1.2.5 / GCC 14.2 was installed only in the build environment. No repository code, model, threshold, vector or frontend changes were made. The baseline snapshot was `/tmp/kirara-search-baseline`; its binary was copied before building current code in the shared isolated musl target directory. These are native musl-target builds on Debian, not the workflow's cross-container image.

| Measurement | Musl baseline | Musl current | Increment |
|---|---:|---:|---:|
| Binary, unstripped release | 29,710,472 B | 40,640,224 B | 10,929,752 B = 10.4234 MiB (+36.79%) |
| Binary gzip level 9 | 10,400,782 B | 16,740,780 B | 6,339,998 B = 6.0463 MiB (+60.96%) |
| Release `tar czf`, gzip default 6 | 10,489,401 B | 16,837,136 B | 6,347,735 B = 6.0537 MiB (+60.52%) |

The baseline archive contains the binary; the current archive additionally carries the model MIT license and provenance, matching the release workflow. Both sides use the same compression level within each row. These figures are separate from the GNU-target measurements and must not be mixed with the GNU baseline or its deterministic gzip-9 archive sample. The measured musl package/raw increments are within the initial 8/12 MiB engineering budgets, while the roughly 60.5% archive growth remains substantial.

`file` identifies both binaries as x86-64 static-PIE executables. The current musl binary SHA256 is `297528ab5abbab6b6944544ffde46e5b54cee41d40db4102534f79c36acee09f`; baseline SHA256 is `10c4be59525374ada0a91a87cd5811eb2cb66162aff8f17e814ce12be9f1d747`. Both `--help` executions succeed. An isolated fresh-database HTTP smoke test returns zero configured sites and successfully runs native catalog semantics (`semantic_status: used`, 163 matches for 无损音乐, 83.64 ms first catalog request).

The full existing synthetic HTTP probe also passes **31/31** checks on this actual musl binary, with two-core affinity on the Intel N150 and no scheduled task executions. Its fixture remains 300 local sites, 150 sign-in tasks, 150 brush tasks and 100,000 history records; it does not substitute for the larger planned 1,000-site/10,000-task scenario. Warm lexical p95 is 11.80 ms; warm hybrid p95 45.95 ms; cold hybrid 76.09 ms. Sampled process peak RSS is 44,372 KiB, with 28.30 MiB extra RSS above the pre-search sample. Four-concurrent hybrid requests all succeed but range 134.32–251.24 ms (12 requests; nearest-rank p95 251.24 ms), so that small concurrent sample exceeds the 200 ms target. A 100,000-record rare text search takes 11.07 s; the admitted concurrent history scan takes 11.60 s while seven excess scans receive 503 and ordinary searches remain available.

The root report retains these observations in `doc/validation/server-smart-search/size-musl.json` and `http-musl.json`. Temporary smoke/probe servers were stopped by their cleanup handlers and no musl application process remains. Windows/macOS/ARM64 execution and the independent semantic-quality gap remain separate open gates; this new evidence establishes actual x86-64 musl application build, execution, packaging and bounded-fixture behavior only.


**Post-cache musl snapshot and exact ranking regression**

A subsequent semantic-preserving cache optimization replaces the text cache's exclusive read lock with shared reads, computes misses outside the write lock, and reuses expanded public descriptions and per-query concept normalization. The existing 4096-entry/512-byte bounds, model, templates, vectors, threshold and ranking rules remain unchanged. The prior musl observations above are retained as the pre-optimization snapshot.

After the core freeze, all 30 queries of the existing third fixture were rerun. Every complete ordered result-ID array and semantic status is exactly equal to the prior final regression; this is a regression check, not a new independent holdout. Thus the native NDCG@10 0.8224324743, Precision@5 0.92 and 0/10 unrelated recall remain unchanged. The new `src/search/mod.rs` SHA256 is `86874b79d50495f9c06696b40613f762961be1e6d41800e363cc2949947422bc`; the semantic module, encoder and manifest hashes remain as listed above. Source hashes were checked before and after evaluation.

Only the current musl application was rebuilt with the same target, profile, compiler and shared build directory; the saved baseline binary is unchanged. Current binary SHA256 is `ee21e6329baecd71f95a1d158422d4ecde0adbf4391c830ad569944e7736789d`.

| Measurement | Same musl baseline | Post-cache musl current | Increment |
|---|---:|---:|---:|
| Binary, unstripped release | 29,710,472 B | 40,683,680 B | 10,973,208 B = 10.46487 MiB |
| Binary gzip level 9 | 10,400,782 B | 16,748,254 B | 6,347,472 B = 6.05342 MiB |
| Release `tar czf`, gzip default 6 | 10,489,401 B | 16,845,849 B | 6,356,448 B = 6.06198 MiB (+60.60%) |

The final post-cache musl HTTP probe ran once after other builds/compression stopped, with the same two-core affinity and synthetic fixture. All **31/31** checks passed. Warm lexical p95 is **9.75 ms**, warm hybrid p95 **21.11 ms**, and cold hybrid **77.37 ms**. All 12 four-concurrent hybrid requests return HTTP 200 with semantics used, ranging **110.25–149.99 ms**; nearest-rank p95/max is **149.99 ms**, compared with the preserved pre-cache **251.24 ms**. This sample meets the 200 ms concurrency target for the bounded fixture, without establishing the larger planned production workload gate.

Sampled peak RSS is **46,700 KiB**, with **28.64 MiB** extra over the initial 17,368 KiB sample. A rare 100,000-row history scan still takes **10.03 s**, a status scan **9.78 s**, and the admitted concurrent scan **11.27 s**; seven excess history scans receive HTTP 503 in 2.50–7.43 ms while ordinary searches remain eligible. The cache optimization does not resolve that separate history-scan limitation. Temporary application processes were confirmed stopped after the probe.

Raw post-cache measurements and exact-ranking verification were delivered to the root report from `/tmp/kirara-musl-size/{metrics.json,http-full.json,third-post-cache-verification.json,third-post-cache-results.json}`. The pre-cache binary, measurements and prior full ranking output remain separately preserved under `/tmp/kirara-musl-size/prior-cache-optimization/`. The root's checked-in validation artifacts retain the durable final/prior observations. No model, asset, threshold, template or relevance judgment was changed in this follow-up.


**Final semantic-cache synchronization snapshot**

A subsequent GNU HTTP run caught two warm concurrent requests falling back to `busy` despite HTTP 200. The semantic state lock had also covered cache reads and catalog dot products. The core separated immutable loaded assets, short query-cache access and exclusive cold initialization/embedding; dot products now run outside these locks. This synchronization-only correction preserves the 64-entry/five-minute query cache and does not change scoring, model, vectors, thresholds or labels. Earlier musl snapshots above remain preserved; their successful samples did not establish absence of this race.

The existing third fixture was rerun once after freeze and again all **30 complete ranked-ID arrays and semantic statuses are exactly unchanged**. This is regression evidence, not a new holdout. Final `src/search/semantic.rs` SHA256 is `7eb8e845fa929eb7606dfaec623d4485fa18b03024bd86cad955889ac77e830a`; the post-cache `mod.rs`, encoder and manifest hashes remain unchanged. Exact proof and outputs were delivered as `third-final-verification.json` and `third-final-results.json` in the temporary measurement directory.

The final musl rebuild uses the unchanged baseline and build flags. Final binary SHA256 is `474a9e61ef2e261b1da051e6d111f1a898765a6fd16ef55a652c45979131b86e`. Raw size is **40,678,400 B**, gzip-9 binary **16,750,074 B**, and default-gzip-6 release archive with notices **16,847,615 B**. Corresponding increments over the same baseline are **10,967,928 B (10.45983 MiB)**, **6,349,292 B (6.05516 MiB)** and **6,358,214 B (6.06367 MiB, +60.62%)**.

One final amended two-core HTTP probe passes **32/32** checks, including the new requirement that all warm concurrent hybrid requests retain semantics and identical totals/page IDs. All 12 concurrent requests satisfy that requirement; latency ranges **126.10–138.71 ms**, nearest-rank p95/max **138.71 ms**. Warm lexical p95 is **8.92 ms**, warm hybrid **19.05 ms**, and cold hybrid **73.26 ms**. Sampled peak RSS is **47,012 KiB**, extra **28.94 MiB**. The 100,000-row rare/status history scans still take **10.30/9.85 s**, a separate remaining limitation. The process was stopped after the probe. The larger design-scale probe is reported separately by the root/API report; this run retains the same 300-site/300-task fixture.

The preceding post-cache musl binary and all observations are preserved under `/tmp/kirara-musl-size/prior-semantic-cache-optimization/`, in addition to the original pre-cache snapshot. Final raw results remain `/tmp/kirara-musl-size/metrics.json` and `http-full.json` for the root's durable validation artifacts. No further code or asset changes were made by this validation task.
