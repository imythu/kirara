`search-encoder` is the native CPU adaptation of the MIT ternlight mini 0.1.1 engine at revision `c6d2c0a35d14c574ed2898b3dbf95977bca07208`. The original `format.rs`, `kernels.rs`, `inference.rs`, `model.rs`, and `tokenizer.rs` are retained. Changes are confined to embedded asset paths, the native adapter in `lib.rs`, feature selection, error boundaries, and the shared concept expansion. Upstream copyright and MIT terms are in [LICENSE](LICENSE).

The published model is 2 transformer layers, hidden width 256, 4 heads, FFN width 1024, output width 384, maximum 128 tokens. It uses int4 token embeddings and ternary transformer matrices, with float32 output projection and normalization. The tokenizer is the upstream BERT uncased WordPiece asset. This is an English-trained embedding model; Chinese domain search is assisted by the shared, reviewable concept mapping.

`Encoder::new()` initializes shared read-only weights and tokenizer once. `Encoder::embed(text)` returns a normalized 384-element vector and does **not** expand concepts. Runtime queries and document generation both explicitly call `expand_semantic_text(text)` once. There is no Node, WASM engine, model download, accelerator, or external process in the server runtime. The Hugging Face tokenizer dependency has default features disabled (no native oniguruma or C++ suffix-array trainer); its feature named `unstable_wasm` selects the portable regex implementation and does not add a WASM execution runtime.

From the repository root:

```sh
# Maintainer import only; reads public site JSON and exact PTD hostname evidence.
python3 crates/search-encoder/tools/import.py --source ../pt-sites
# Recompute public phonetics and all 616 vectors using the shipped native model.
cargo run -p search-encoder --release --example assets -- build
# Offline validation of input hashes, row mapping, endian shape and normalization.
cargo run -p search-encoder --release --example assets -- verify
# Domain probe and native performance benchmark.
cargo run -p search-encoder --release --example assets -- eval
cargo run -p search-encoder --release --example benchmark
cargo test -p search-encoder --release
```

The importer does not modify the source project. It materializes explicit PTD-to-catalog ID pairs, including `mteam` → `m-team`, with hosts taken only from exact official URLs and the checked-in PTD hostname table. It never strips punctuation to infer identity. The source commit, deterministic sorted row IDs, input hashes, model/tokenizer hashes, engine revision, template, expansion version, endian format and vector hash are in the manifest. A source-data refresh requires import followed by build. Ordinary server builds consume Git assets and need no neighboring project.

For maintenance verification against the pinned published reference only:

```sh
node crates/search-encoder/tools/reference.cjs ../pt-sites/node_modules/@ternlight/mini extract
node crates/search-encoder/tools/reference.cjs ../pt-sites/node_modules/@ternlight/mini reference
cargo test -p search-encoder --release
```

`extract` verifies the package name/version/license, model header, embedded trailing digest and whole-file pinned SHA256 before writing raw weights. The npm tarball, reference WASM digest, tokenizer source and model hashes are in `assets/search/models/mini/provenance.json`. Node is needed only to repeat this reference extraction; the checked-in assets build offline without it. The public model/tokenizer MIT license is shipped with the asset directory.

`evaluation.json`, `evaluation-heldout-v2.json` and `evaluation-heldout-v3.json` retain three successive 30-query fixtures with explicit graded judgments derived from public domain metadata. The first two became diagnostic sets after general parser defects were identified; the third was evaluated only after the final parser/ranking freeze. `tools/evaluate.mjs` compares the upstream lexical/concept channel, existing pt-sites hybrid and new native service results. It requires a maintainer's adjacent pt-sites reference installation; this is not a production dependency. See `doc/search-encoder-validation.md` for actual measurements and limitations.

`semantic_rank` is a diagnostic example that reads a JSON array of queries on stdin and ranks raw native vectors at the fixed 0.65 cutoff without the service classifier. It is not production search and intentionally exposes why unmapped Chinese needs the service concept gate.
