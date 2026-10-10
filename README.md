# argentina-normativa-cli

Command-line scraper for the provincial laws published on
[argentina.gob.ar/normativa](https://www.argentina.gob.ar/normativa). It drives a headless Chrome to:

- list the provinces available in the search form,
- search the laws (`Ley`) of a province by keyword,
- download a single law as a PDF.

## Requirements

- [Rust](https://www.rust-lang.org/tools/install) (edition 2024, so a recent stable toolchain)
- A Chromium-based browser installed on the system: Chromium, Google Chrome or Brave. The scraper launches it in
  headless mode; no browser is bundled. See [Choosing the browser](#choosing-the-browser).

### Choosing the browser

The browser is auto-detected. In order, the scraper uses:

1. The executable whose full path is in the `CHROME` environment variable.
2. The first of `chrome`, `chrome-browser`, `google-chrome-stable`, `chromium`, `chromium-browser` or `msedge`
   found on your `PATH`, then a few well-known install locations (e.g. `/opt/google/chrome`).

Chromium and Google Chrome are found this way once installed. **Brave is not auto-detected**; point `CHROME` at it:

```bash
CHROME=/usr/bin/brave argentina-normativa-cli list   # use the output of `which brave` if it differs
```

`CHROME` has to be a full path to an existing file, not just a command name. Without any browser the command fails
with `Could not auto detect a chrome executable`.

## Build

```bash
cargo build --release
```

The binary is `target/release/argentina-normativa-cli`. The examples below assume it is on your `PATH`.

## Usage

### `list`

Prints the provinces of the "Elegí una provincia" select as a JSON array. Use these exact names with `query provinces`.

```bash
argentina-normativa-cli list
```

#### `list national`

Prints the values the national search form accepts, as JSON arrays, to use with `query national`.

```bash
argentina-normativa-cli list national agencies   # --agency: 1690 exact upper-case names
argentina-normativa-cli list national law-type   # --law-type: slugs such as leyes, decretos (no browser needed)
argentina-normativa-cli list national years      # --year: 2026 down to 1853
```

### `query`

Searches laws and prints one JSON object per line. Pick what to search with a subcommand: `provinces` or `national`.

#### `query provinces`

Searches the laws of one province.

```bash
argentina-normativa-cli query provinces --province "Córdoba" --query "impuesto tasa"
```

| Option | Description |
| --- | --- |
| `--province <name>` | Province name exactly as printed by `list` (required). |
| `--query <keywords>` | Keywords for "Buscá por palabras clave". All words must match. |

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
| `--max-pages <n>` | Stop after `n` result pages (50 rows each, 10 s apart). Default 20; a warning on stderr says when the result was truncated. |

Output lines have the same shape as the provincial ones, with `"provincia": null`, `"jurisdiccion": "nacional"`, an
extra `organismo` (issuing agency) and a `ley` slug like `norma-431078`. `fetch` does not support national norms yet.

### `fetch`

Opens a law page, prints its province, title and status as JSON, then follows the "Ver norma" button and saves the
law text as a PDF.

```bash
argentina-normativa-cli fetch --jurisdiction provincial --law ley-14709-123456789-0abc-defg-907-4100bvorpyel
```

| Option | Description |
| --- | --- |
| `--jurisdiction <provincial\|nacional>` | Jurisdiction segment of the law URL. Only `provincial` is supported for now; `nacional` exits with an error. |
| `--law <ley>` | Law slug, the last segment of the law URL. Letters, digits and dashes only. |
| `--output <file_path>` | Where to write the PDF. Defaults to `<ley>.pdf` in the current directory. The parent directory must exist. |

Output:

```json
{
  "provincia": "Buenos Aires",
  "jurisdiccion": "provincial",
  "titulo": "Ley 14709",
  "ley": "ley-14709-123456789-0abc-defg-907-4100bvorpyel",
  "estado": "Vigente, de alcance general",
  "url": "https://www.argentina.gob.ar/normativa/provincial/ley-14709-123456789-0abc-defg-907-4100bvorpyel",
  "pdf": "ley-14709-123456789-0abc-defg-907-4100bvorpyel.pdf"
}
```

An existing file at the output path is overwritten.

## Output and logging

Only data goes to **stdout** (JSON or JSON lines), so it can be piped, for example into `jq`. Logs go to **stderr**;
control them with `RUST_LOG` (default `info,chromiumoxide=error`).

```bash
argentina-normativa-cli query provinces --province "Córdoba" --query "impuesto" | jq -r '.url'
```

## Good to know

- The site's `robots.txt` asks for a 10 second crawl delay, and the scraper waits that long between result pages
  and between the two requests of `fetch`. A query with many pages therefore takes a while.
- Provincial search is limited to the `Ley` norm type, which is the only type the site accepts for provinces.
- A province name that matches nothing prints no results and logs a warning; check the spelling (including
  accents) against `list`.

## Development

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
```

Tests are offline: they parse saved pages from `tests/fixtures/`. See `CLAUDE.md` for architecture notes.
