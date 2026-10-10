# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

`argentina-normativa-cli`: a Rust (edition 2024) CLI that drives headless Chrome against
https://www.argentina.gob.ar/normativa to collect **provincial laws** (`Ley`). It needs a system Chrome
(`chromiumoxide` launches it; no bundled browser). Law PDFs written by `fetch` land in the working directory.

## Commands

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all                       # offline, fixture-based
cargo test parse_law_page              # single test (substring match on the test name)
cargo build --release                  # binary: target/release/argentina-normativa-cli

argentina-normativa-cli list                                         # provinces as a JSON array
argentina-normativa-cli list national <agencies|law-type|years>      # values for `query national` flags, JSON array
argentina-normativa-cli query --province "Córdoba" --query "impuesto tasa"   # JSON lines
argentina-normativa-cli query national --law-type decretos --year 2024 --query "impuesto"   # national, JSON lines
argentina-normativa-cli fetch --jurisdiction provincial --law <ley-slug> [--output file.pdf]
```

`cargo clippy`/`cargo test` do not produce `target/release/...`; build it explicitly before live runs.
Live runs hit the real site and are slow by design (see crawl delay below).

## Architecture

Everything lives in `src/main.rs` (CLI, parsers, browser driving, unit tests in one file). Flow per subcommand:
`main` parses args and validates input **before** launching Chrome → `launch_browser` → `run_list | run_query |
run_fetch` → close browser, then return the command's result.

Parsing is kept pure and separate from browsing: `fetch_html` (`page.goto` + `page.content()`) returns HTML, and
`parse_options` / `parse_results` / `parse_law_page` turn it into data with the `scraper` crate. This is what makes
the tests offline: they run the parsers against saved pages in `tests/fixtures/` (`results.html`, `law.html`).
When a selector breaks, refresh the fixture from the live page and fix the parser, not the other way round.

- **stdout is data only** (JSON / JSON lines); all logs go to stderr via `tracing`. Don't `println!` anything else.
- The `chromiumoxide` handler task must keep polling `handler.next()` and ignore errors: Chrome emits CDP
  messages the crate can't decode, and breaking on the first one kills the browser connection.
- Law URL shape: `/normativa/<provincial|nacional>/<ley-slug>`; `parse_law_href` / `law_url` convert both ways.
  Only `provincial` is verified live. `fetch --jurisdiction nacional` is deliberately rejected in `validate_fetch`.
- `--law` becomes both a URL path and the default PDF file name, so `validate_law_slug` allowlists
  `[A-Za-z0-9-]+`. Keep new user input going into URLs/paths behind a similar check.

## Site behaviour that is easy to get wrong

- Searching works with plain GETs: `/normativa?provincia=<name>&jurisdiccion=provincial&tipo_norma=Ley&texto=<kw>&limit=50&offset=<page>`.
  `offset` is a **1-based page number**, not a row offset. Cloudflare Turnstile only guards the POST form submit, so the
  scraper navigates by URL instead of filling the form.
- National search (`query national`) is also plain GET: `/normativa?jurisdiccion=nacional&tipo_norma=&numero=&anio=&dependencia=&publicacion_desde=&publicacion_hasta=&texto=&s=1&page=<n>`.
  Send **every** param, even empty. `page` is **0-based**, 50 rows per page, no `limit`/`offset`. Dates must be ISO
  `YYYY-MM-DD` (`dd-mm-aaaa`, the form placeholder, returns nothing). `tipo_norma` is a slug (`leyes`, `decretos`, ...;
  see `LAW_TYPES`). `leyes` + `anio` returns a page with no results block, so `validate_national` rejects it.
  Counter is `div.infoleg-search-results-count` ("N normas encontradas en P páginas", pages only as text); rows link to
  `/normativa/nacional/norma-<id>` and the issuing agency is the `p.small` in the Normativa cell.
  The national page has two forms with a `dependencia` select (1690 agencies each); `list national` scopes to
  `form#infoleg-normativa-search-form` to avoid duplicates. `list national law-type` prints `LAW_TYPES`, no Chrome.
  `query national` stops after `--max-pages` (default `DEFAULT_MAX_PAGES` = 20) and warns when truncated; provincial is uncapped.
- `tipo_norma` is mandatory (empty/`todas`/`*` return nothing); provincial search is fixed to `Ley`.
- `texto` is AND-semantics across words. Province names are exact, accented strings from `list` (e.g. `Córdoba`).
- `robots.txt` sets `Crawl-delay: 10`; the code sleeps `CRAWL_DELAY` between result pages and between the two
  requests of `fetch`. Don't remove it or parallelise requests.
- `fetch` clicks `a.btn.btn-primary` ("Ver norma"), which navigates to `<law url>/actualizacion` (the law text), and prints
  that page to PDF. The click only *starts* navigation, so `wait_for_url_suffix` polls the URL before printing.

## Conventions

- Git: never commit to `main`; use `feature/`, `fix/`, `enhancement/` branches and atomic Conventional Commits
  (`<type>(<scope>): <description>`, first line under 40 chars; type `feature`, not `feat`).
- `Cargo.lock` is committed (binary crate). Verify any new crate and version on crates.io before adding it.
