<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# templates/

Assets vendored into the self-contained interactive HTML output.

## `cytoscape.min.js`

Cytoscape.js v3.30.2, the graph rendering library embedded inline into the
generated HTML so the output works fully offline (no CDN, no network).

- Upstream: <https://js.org/cytoscape> / <https://github.com/cytoscape/cytoscape.js>
- License: MIT (Copyright (c) 2016-2024, The Cytoscape Consortium). The full MIT
  license text is preserved in the banner comment at the top of the minified
  file. Cytoscape.js's MIT license is compatible with this crate's
  GPL-3.0-or-later: the GPL output may bundle an MIT-licensed library.

If this file is ever missing (e.g. a checkout without it), the HTML renderer
falls back to loading Cytoscape from a CDN `<script src=…>` and the output is no
longer offline-self-contained until the asset is restored. To re-vendor:

```sh
curl -fsSL https://cdnjs.cloudflare.com/ajax/libs/cytoscape/3.30.2/cytoscape.min.js \
  -o templates/cytoscape.min.js
```

## `viewer.html`

The HTML shell. It contains three placeholders the renderer substitutes:

- `/*__CYTOSCAPE_JS__*/` — replaced with the inline contents of
  `cytoscape.min.js` (or a CDN `<script>` fallback).
- `/*__GRAPH_JSON__*/` — replaced with the project's `GraphModel` as JSON.
- `__TITLE__` — replaced with the project title.
