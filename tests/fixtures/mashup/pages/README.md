# Page fixtures

The top-level specs cover individual v1 elements. `bench_a`, `bench_h`, `bench_j`,
`bench_k`, `bench_l`, `bench_m`, `bench_n`, `bench_o`, `bench_p`, `bench_s` and `bench_q1`..`bench_q4` mirror the deployment benchmarks with neutral Acme names.
`bench_solution` supplies only their offline service contracts, DataShapes and status
snapshots; it never contacts a server. A includes projected sparklines, a secondary combo axis
and numeric StateDefinition dots; Q4 requests native first-row selection. The status image names are neutral placeholders.

`bench_l` generates a collection cell page; its golden records it under `cells`.

See [worked examples](../../../../documentation/PAGE_SPECS.md#worked-examples) for
kit mappings and known differences. The golden policy permits deliberate v1 improvements;
legacy layout and explanation goldens remain unchanged.

`kit_structures/` holds small hand-written Acme mashup contents that reproduce only the container
structure the page-spec derivation recognises (a filter column, a side panel, a section gallery,
an admin shell, tabs with a function widget). They contain no content copied from any vendor kit.
The `mashup spec` tests derive a spec from each, build it against `bench_solution` and compare the
widget tree shape of the original and the rebuilt page. See PAGE_SPECS.md, "Deriving a spec".

`kit_fidelity/` covers selected/indented navigation, native pie palette styles, a centred ticker,
and generated image/status cells. Acme.MediaData provides only a synthetic offline contract.
`bench_clean/` is the synthetic Acme A page under `tokens: clean`, with native card and page
styles held to a golden for later visual review.
