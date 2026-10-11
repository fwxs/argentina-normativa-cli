# argentina-normativa-cli

Command-line scraper for the provincial laws published on
[argentina.gob.ar/normativa](https://www.argentina.gob.ar/normativa). It can:

- list the provinces available in the search form,
- search the laws (`Ley`) of a province by keyword,
- save the text of a single law as Markdown.

## Requirements

- [Rust](https://www.rust-lang.org/tools/install) (edition 2024, so a recent stable toolchain)

No browser is needed: the site's pages are server-rendered, so the scraper reads them with plain HTTPS requests.

## Build

```bash
cargo build --release
```

The binary is `target/release/argentina-normativa-cli`. The examples below assume it is on your `PATH`.

## Usage

### `list`

Prints the values a search form accepts, as JSON arrays. Pick which with a subcommand: `provinces` or `national`.

#### `list provinces`

Prints the provinces of the "Elegí una provincia" select. Use these exact names with `query provinces`.

```bash
argentina-normativa-cli list provinces
```

#### `list national`

Prints the values the national search form accepts, as JSON arrays, to use with `query national`.

```bash
argentina-normativa-cli list national agencies   # --agency: 1690 exact upper-case names
argentina-normativa-cli list national law-type   # --law-type: slugs such as leyes, decretos (no request needed)
argentina-normativa-cli list national years      # --year: 2026 down to 1853
```

### `query`

Searches laws and prints one JSON object per line. Pick what to search with a subcommand: `provinces` or `national`.

#### `query provinces`

Searches the laws of one province. At least one filter is required (an unfiltered search is every law of the province,
thousands of rows).

```bash
argentina-normativa-cli query provinces --province "Córdoba" --query "impuesto tasa"
argentina-normativa-cli query provinces --province "Buenos Aires" --year 2020
argentina-normativa-cli query provinces --province "Buenos Aires" --from-date 2020-01-01 --to-date 2020-12-31
```

| Option | Description |
| --- | --- |
| `--province <name>` | Province name exactly as printed by `list provinces` (required). |
| `--law-number <n>` | "Número": digits only. |
| `--year <yyyy>` | "Año" (1853 or later). |
| `--from-date <YYYY-MM-DD>` | "Publicación desde". |
| `--to-date <YYYY-MM-DD>` | "Publicación hasta". |
| `--query <keywords>` | Keywords for "Buscá por palabras clave". All words must match. |
| `--max-pages <n>` | Stop after `n` result pages (50 rows each, 10 s apart). Without it every page is fetched, one per 10 s, so a broad filter can run for a long time; a warning on stderr says when the result was truncated. |

Example line (wrapped for readability):

```json
{
  "provincia": "Ciudad Autónoma de Buenos Aires",
  "jurisdiccion": "provincial",
  "tipo_norma": "Ley",
  "titulo": "Ley 4492",
  "ley": "ley-4492-123456789-0abc-defg-294-4000xvorpyel",
  "url": "https://www.argentina.gob.ar/normativa/provincial/ley-4492-123456789-0abc-defg-294-4000xvorpyel",
  "fecha_publicacion": "2013-05-02",
  "descripcion": ["Ciudad Autónoma de Buenos Aires", "ALUMBRADO, BARRIDO Y LIMPIEZA-LEYENDA IMPRESA", "..."]
}
```

The `ley` field is the law slug you pass to `fetch`. Results are paged (50 per page) and all pages are fetched.

#### `query national`

Searches national norms (laws, decrees, resolutions, ...) with the filters of the site's national form.
At least one filter is required.

```bash
argentina-normativa-cli query national --law-type decretos --from-date 2024-01-01 --to-date 2024-01-31
argentina-normativa-cli query national --query "impuesto" --agency "MINISTERIO DE ECONOMIA"
```

| Option | Description |
| --- | --- |
| `--law-type <slug>` | "Tipo de norma": `leyes`, `decretos`, `resoluciones`, ... (see `list national law-type`). |
| `--law-number <n>` | "Número": digits only. |
| `--year <yyyy>` | "Año" (see `list national years`). The site finds nothing for `leyes` + year; use the dates for laws. |
| `--agency <name>` | "Organismo o dependencia": exact upper-case name (see `list national agencies`). |
| `--from-date <YYYY-MM-DD>` | "Publicación desde". |
| `--to-date <YYYY-MM-DD>` | "Publicación hasta". |
| `--query <keywords>` | "Buscá por palabras clave". |
| `--max-pages <n>` | Stop after `n` result pages (50 rows each, 10 s apart). Without it every page is fetched, one per 10 s; a warning on stderr says when the result was truncated. |

Output lines have the same shape as the provincial ones, without `provincia`, with `"jurisdiccion": "nacional"`, an
extra `organismo` (issuing agency) and a `ley` slug like `norma-431078`, which `fetch --jurisdiction nacional --law` accepts.

### `fetch`

Opens a law page, prints its details as JSON, then follows the "Ver norma" link and saves the
law text as Markdown (the title as a heading, then one paragraph per block of the law).

```bash
argentina-normativa-cli fetch --jurisdiction provincial --law ley-14709-123456789-0abc-defg-907-4100bvorpyel
argentina-normativa-cli fetch --jurisdiction nacional --law norma-431079
```

| Option | Description |
| --- | --- |
| `--jurisdiction <provincial\|nacional>` | Jurisdiction segment of the law URL. |
| `--law <ley>` | Law slug, the last segment of the law URL. Letters, digits and dashes only; for `nacional` it is `norma-` followed by the number, e.g. `norma-431079`. |
| `--output <file_path>` | Where to write the Markdown. Defaults to `<ley>.md` in the current directory. The parent directory must exist and the file must not. |

Output: the `jurisdiccion` and, under `metadata`, the fields of that jurisdiction.

```json
{
  "jurisdiccion": "provincial",
  "metadata": {
    "provincia": "Buenos Aires",
    "titulo": "Ley 14709",
    "ley": "ley-14709-123456789-0abc-defg-907-4100bvorpyel",
    "estado": "Vigente, de alcance general",
    "url": "https://www.argentina.gob.ar/normativa/provincial/ley-14709-123456789-0abc-defg-907-4100bvorpyel",
    "archivo": "ley-14709-123456789-0abc-defg-907-4100bvorpyel.md"
  }
}
```

A national norm has no `provincia` or `estado`, but an `organismo` (issuing agency):

```json
{
  "jurisdiccion": "nacional",
  "metadata": {
    "titulo": "Decreto 1160/2026",
    "organismo": "PODER EJECUTIVO NACIONAL (P.E.N.)",
    "ley": "norma-431079",
    "url": "https://www.argentina.gob.ar/normativa/nacional/norma-431079",
    "archivo": "norma-431079.md"
  }
}
```

> **Changed output shape:** the details used to be one flat object. Scripts must now read the fields under
> `.metadata` (for example `jq -r .metadata.archivo`).

The saved national text is the updated text when the site offers one (norms that were modified later), otherwise the
original text.

An existing file at the output path is never overwritten: `fetch` fails before any request, and you delete the file to fetch again. A slug the site does not know exits with `law not found`.
`--output` (or the default `<ley>.md`) is checked before any request is sent, so a missing directory or an existing file fails immediately.

> **Changed from the PDF version:** `fetch` now saves Markdown instead of a PDF, and the `pdf` key of the JSON
> output is now `archivo`. A script reading `.pdf` (for example `jq -r .pdf`) gets `null`; read `.archivo` instead.

## Output and logging

Only data goes to **stdout** (JSON or JSON lines), so it can be piped, for example into `jq`. Logs go to **stderr**;
control them with `RUST_LOG` (default `info`).

```bash
argentina-normativa-cli query provinces --province "Córdoba" --query "impuesto" | jq -r '.url'
```

## Good to know

- The site's `robots.txt` asks for a 10 second crawl delay, and the scraper waits that long between result pages
  and between the two requests of `fetch`. A query with many pages therefore takes a while.
- Rate limiting (429), gateway errors (502-504) and timeouts are retried up to twice, waiting at least the crawl
  delay. If a query still fails halfway, the error says which page it stopped on and how many rows were already
  printed, so partial output is never mistaken for a complete result.
- Provincial search is limited to the `Ley` norm type, which is the only type the site accepts for provinces.
- A province name that matches nothing prints no results and logs a warning; check the spelling (including
  accents) against `list provinces`.

## Development

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
```

Tests are offline: they parse saved pages from `tests/fixtures/`. See `CLAUDE.md` for architecture notes.
