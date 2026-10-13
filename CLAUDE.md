# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

`argentina-normativa-cli`: a Rust (edition 2024) CLI that scrapes https://www.argentina.gob.ar/normativa with plain
HTTPS GETs (`reqwest`) to collect **provincial laws** (`Ley`) and national norms. No browser: every page it reads is
server-rendered. Law texts written by `fetch` (Markdown, `<ley>.md`) land in the working directory.

## Commands

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all                       # offline, fixture-based
cargo test parse_law_page              # single test (substring match on the test name)
cargo build --release                  # binary: target/release/argentina-normativa-cli

argentina-normativa-cli list provinces                               # provinces as a JSON array
argentina-normativa-cli list national <agencies|law-type|years>      # values for `query national` flags, JSON array
argentina-normativa-cli query provinces --province "Córdoba" --query "impuesto tasa"   # provincial, JSON lines
argentina-normativa-cli query national --law-type decretos --year 2024 --query "impuesto"   # national, JSON lines
argentina-normativa-cli fetch --jurisdiction provincial --law <ley-slug> [--output file.md]
argentina-normativa-cli fetch --jurisdiction nacional --law norma-431079 [--output file.md]
```

`cargo clippy`/`cargo test` do not produce `target/release/...`; build it explicitly before live runs.
Live runs hit the real site and are slow by design (see crawl delay below).

## Architecture

The crate is a library (`src/lib.rs`) plus a thin binary (`src/main.rs`: parse args, init `tracing`, call `run`).
Modules, one domain each; unit tests sit in a `#[cfg(test)]` module at the bottom of the file they cover:

- `cli` clap types (`Cli`, `Command`, `NationalFilters`, ...) · `model` shared types (`Jurisdiccion`, `Normativa`, ...)
- `site` constants (`LAW_TYPES`, `CRAWL_DELAY`, ...) and url builders · `validate` input checks
- `parse/{mod,results,law,national,markdown}` pure HTML parsers · `http` client + `fetch_html` · `output` stdout writers
- `commands/{list,query,fetch}` one runner per subcommand

Flow per subcommand: `lib::run` validates input **before** any request → `build_client` →
`run_list_provinces | run_list_national | run_query_provinces | run_query_national | run_fetch`, each taking `&reqwest::Client`.
Keep items `pub(crate)` unless `main.rs` needs them.

Parsing is kept pure and separate from fetching: `fetch_html` (GET, non-2xx is an error) returns HTML, and
`parse_options` / `parse_results` / `parse_law_page` / `parse_law_text` turn it into data with the `scraper` crate.
This is what makes the tests offline: they run the parsers against saved pages in `tests/fixtures/`
(`results.html`, `law.html`, `law_text.html`, ...).
When a selector breaks, refresh the fixture from the live page and fix the parser, not the other way round.

- **stdout is data only** (JSON / JSON lines); all logs go to stderr via `tracing`. Don't `println!` anything else.
- Law URL shape: `/normativa/<provincial|nacional>/<ley-slug>`; `parse_law_href` / `law_url` convert both ways.
  Both jurisdictions are verified live for `fetch`; a national slug is `norma-<id>` (`validate_national_slug`).
- `--law` becomes both a URL path and the default Markdown file name, so `validate_law_slug` allowlists
  `[A-Za-z0-9_-]+`. Keep new user input going into URLs/paths behind a similar check.

## Site behaviour that is easy to get wrong

- Searching works with plain GETs: `/normativa?provincia=<name>&jurisdiccion=provincial&tipo_norma=Ley&texto=<kw>&limit=50&offset=<page>`.
  `offset` is a **1-based page number**, not a row offset. Cloudflare Turnstile only guards the POST form submit, so the
  scraper navigates by URL instead of filling the form.
  Optional filters are added only when set, with the **provincial** form's names (verified live 2026-10-10, they differ
  from national): year is `sancion` (`anio` is silently ignored), `numero`, and flat ISO `publicacion_desde` /
  `publicacion_hasta` (the form's `publicacion_desde[date]` returns no rows, `dd-mm-yyyy` filters nothing); `texto`
  may be omitted. `validate_provinces` needs at least one filter: a bare province is thousands of rows (Buenos Aires: 88 pages).
- National search (`query national`) is also plain GET: `/normativa?jurisdiccion=nacional&tipo_norma=&numero=&anio=&dependencia=&publicacion_desde=&publicacion_hasta=&texto=&s=1&page=<n>`.
  Send **every** param, even empty. `page` is **0-based**, 50 rows per page, no `limit`/`offset`. Dates must be ISO
  `YYYY-MM-DD` (`dd-mm-aaaa`, the form placeholder, returns nothing). `tipo_norma` is a slug (`leyes`, `decretos`, ...;
  see `LAW_TYPES`). `leyes` + `anio` returns a page with no results block, so `validate_national` rejects it.
  Counter is `div.infoleg-search-results-count` ("N normas encontradas en P páginas", pages only as text); rows link to
  `/normativa/nacional/norma-<id>` and the issuing agency is the `p.small` in the Normativa cell.
  The national page has two forms with a `dependencia` select (1690 agencies each); `list national` scopes to
  `form#infoleg-normativa-search-form` to avoid duplicates. `list national law-type` prints `LAW_TYPES`, no request.
  `query` (both) stops after the optional `--max-pages` and warns when truncated; without it, it is uncapped.
- `tipo_norma` is mandatory (empty/`todas`/`*` return nothing); provincial search is fixed to `Ley`.
- `texto` is AND-semantics across words. Province names are exact, accented strings from `list provinces` (e.g. `Córdoba`).
- `robots.txt` sets `Crawl-delay: 10`; the code sleeps `CRAWL_DELAY` between result pages and between the two
  requests of `fetch`. Don't remove it or parallelise requests.
- `fetch_html` retries 429/502/503/504 and timeouts/connect errors up to 3 tries in total, sequentially, waiting at
  least `CRAWL_DELAY` (doubling, honouring `Retry-After`, capped at 60 s). `query` rows are printed as pages arrive,
  so a failure reports which page it died on and how many rows were already printed.
- `fetch` reads the `href`s of `a.btn.btn-primary` ("Ver norma"); `parse_law_text_link` only accepts exactly
  `law_text_path(<jurisdiction>, <law>, kind)` for the kinds in its `LawPageLayout` (`parse/law`, preferred first;
  it and the detail selectors beside it are the one place with each jurisdiction's selectors), and the client only
  follows `https` redirects to the site's own host (`is_allowed_redirect`), so neither the page nor a `Location` header can
  send the second GET elsewhere. Provincial: `<law path>/actualizacion`, text in `article.m-t-2`. National: `/actualizacion`
  ("Ver texto actualizado") exists only for modified norms and is a real 404 otherwise, so it is used only when the page links it;
  else `/texto` ("Ver norma original"). National text is in `div.infoleg-norma-body`, details have no province/status but an
  `organismo` (`p.lead.m-b-0`). `fetch` prints `LawDetails`, an enum that nests each jurisdiction's fields under `metadata`. The text is rendered to Markdown by `parse_law_text`
  (`parse/markdown`: block walker for paragraphs, headings, lists, tables; escapes Markdown specials; one `<br>` is a space,
  two in a row end the paragraph, which national texts rely on). A missing slug is a real HTTP 404 (`FetchError::Status`, see `http`), which `run_fetch`
  reports as "law not found" ("law text not found" for the second GET).
  `--output` (or the default `<ley>.md`) is checked in `validate_fetch` (parent directory exists, not a directory, file does not exist: never overwritten) before any request.

## Conventions

- Git: never commit to `main`; use `feature/`, `fix/`, `enhancement/` branches and atomic Conventional Commits
  (`<type>(<scope>): <description>`, first line under 40 chars; type `feature`, not `feat`).
- `Cargo.lock` is committed (binary crate). Verify any new crate and version on crates.io before adding it.
