These are offline server assets. Do not copy this directory into frontend/public bundles.

`catalog.json` is the whitelisted public snapshot of 308 sites from pt-sites commit `dcd825aa1684680a52e6a85b42fb410b5551e156`. It contains public names/aliases, exact verified hosts, explicit PTD identity mappings, domain metadata, concise descriptions, source references and phonetics. Source collection status (including beginner descriptions mentioning invitations) is a historical snapshot, not live access information. No user configuration, credentials or private query history enters these files.

`concepts.json` is the shared query/document expansion vocabulary. `catalog-vectors.bin` contains two 384-dimensional normalized float32 little-endian vectors per ID, in `manifest.json` row order. `models/mini` contains actual pretrained mini weights and the fixed tokenizer, with license and provenance. Model asset use during compilation and query execution is offline; ordinary Cargo dependency resolution is unchanged.

Regeneration and validation commands are in `crates/search-encoder/README.md`. Do not update the model, tokenizer, concepts, template, catalog or vector file independently. Rebuild the manifest and vectors as a compatible set. `evaluation.json` contains public, synthetic evaluation queries and source-derived relevance judgments, not user queries.
