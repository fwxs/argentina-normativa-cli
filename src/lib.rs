//! Scrapes "Normativas" from argentina.gob.ar/normativa, driving headless Chrome.
//! `list` prints the provinces of the search form; `query` prints matching provincial laws (or, with
//! `query national`, national norms) as JSON lines;
//! `fetch` prints one law's details as JSON and saves its text as a PDF.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::page::PrintToPdfParams;
use chromiumoxide::page::Page;
use clap::builder::PossibleValuesParser;
use clap::{Args, Parser, Subcommand, ValueEnum, value_parser};
use futures::StreamExt;
use scraper::{Html, Selector};
use serde::Serialize;
use tokio::task::JoinHandle;
use url::Url;

const SITE_ORIGIN: &str = "https://www.argentina.gob.ar";
const SEARCH_PATH: &str = "/normativa";
const PAGE_SIZE: &str = "50";
// The site's provincial search only accepts this norm type (the form field is disabled with it).
const TIPO_NORMA: &str = "Ley";
// `tipo_norma` slugs of the national search form; an empty value searches every type.
const LAW_TYPES: &[&str] = &[
    "leyes",
    "decretos",
    "decisiones_administrativas",
    "resoluciones",
    "disposiciones",
    "acordadas",
    "actas",
    "actuaciones",
    "acuerdos",
    "circulares",
    "comunicaciones",
    "comunicados",
    "convenios",
    "decisiones",
    "decretos_ley",
    "directivas",
    "instrucciones",
    "interpretacion",
    "laudos",
    "memorandums",
    "misiones",
    "notas",
    "notas_externas",
    "protocolos",
    "providencias",
    "recomendaciones",
];
// Default for `query national --max-pages`: a lone broad filter (`--law-type decretos`) is thousands of
// rows, and every page costs a crawl delay.
const DEFAULT_MAX_PAGES: u32 = 20;
// robots.txt asks for `Crawl-delay: 10`.
const CRAWL_DELAY: Duration = Duration::from_secs(10);

const PROVINCE_SELECTOR: &str = ".label";
const LAW_TITLE_SELECTOR: &str = "h1.h5";
const LAW_STATUS_SELECTOR: &str = "p.m-b-0:nth-child(4) > small:nth-child(1)";
const VIEW_LAW_BUTTON_SELECTOR: &str = "a.btn.btn-primary";
// The "Ver norma" button leads to `<law url>/actualizacion`, the page holding the law text.
const VIEW_LAW_PATH_SUFFIX: &str = "/actualizacion";

/// Scraper for provincial laws published on argentina.gob.ar/normativa.
#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Print the provinces of the "Elegí una provincia" select box as a JSON array.
    List {
        #[command(subcommand)]
        scope: Option<ListScope>,
    },
    /// Search the laws of one province (or, with `national`, national norms) as JSON lines on stdout.
    #[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
    Query {
        /// Province name exactly as printed by `list`.
        #[arg(long, required = true)]
        province: Option<String>,
        /// Keywords for "Buscá por palabras clave", e.g. "impuesto tasa".
        #[arg(long, required = true)]
        query: Option<String>,
        #[command(subcommand)]
        scope: Option<QueryScope>,
    },
    /// Print a law's province, title and status as JSON and save its text as a PDF (default `<law>.pdf`).
    Fetch {
        /// Jurisdiction segment of the law url (only `provincial` is supported for now).
        #[arg(long)]
        jurisdiction: Jurisdiccion,
        /// Law slug, last segment of the law url, e.g. "ley-11035-123456789-0abc-defg-373-0000svorpyel".
        #[arg(long)]
        law: String,
        /// File the PDF is written to; defaults to `<law>.pdf` in the current directory.
        #[arg(long, value_name = "FILE_PATH")]
        output: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ListScope {
    /// Print the values the national search form accepts, as a JSON array.
    National {
        #[command(subcommand)]
        what: NationalList,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Subcommand)]
pub enum NationalList {
    /// "Organismo o dependencia" names, for `query national --agency`.
    Agencies,
    /// "Tipo de norma" slugs, for `query national --law-type`.
    LawType,
    /// "Año" values (newest first), for `query national --year`.
    Years,
}

#[derive(Debug, Subcommand)]
pub enum QueryScope {
    /// Search national norms with the filters of the site's national form.
    National(NationalArgs),
}

/// Arguments of `query national`: the form filters plus a crawl cap.
#[derive(Debug, Args, PartialEq)]
pub struct NationalArgs {
    #[command(flatten)]
    filters: NationalFilters,
    /// Stop after this many result pages (50 rows each, one crawl delay per page).
    #[arg(long, default_value_t = DEFAULT_MAX_PAGES, value_parser = value_parser!(u32).range(1..))]
    max_pages: u32,
}

/// clap parser for free-text filters: trims, and rejects empty values so they can't pass for a filter.
fn non_empty_trimmed(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("value must not be empty".to_owned());
    }
    Ok(trimmed.to_owned())
}

/// Fields of the national search form; every one is optional but at least one must be given.
#[derive(Debug, Default, Args, PartialEq)]
pub struct NationalFilters {
    /// "Tipo de norma": site slug, e.g. leyes, decretos, resoluciones.
    #[arg(long, value_parser = PossibleValuesParser::new(LAW_TYPES.iter().copied()))]
    law_type: Option<String>,
    /// "Número": norm number, digits only.
    #[arg(long, value_parser = value_parser!(u64))]
    law_number: Option<u64>,
    /// "Año": four-digit year (the site's year select starts at 1853).
    #[arg(long, value_parser = value_parser!(u16).range(1853..))]
    year: Option<u16>,
    /// "Organismo o dependencia": exact (upper-case) agency name, see `list national agencies`.
    #[arg(long, value_parser = non_empty_trimmed)]
    agency: Option<String>,
    /// "Publicación desde": YYYY-MM-DD.
    #[arg(long, value_name = "YYYY-MM-DD", value_parser = non_empty_trimmed)]
    from_date: Option<String>,
    /// "Publicación hasta": YYYY-MM-DD.
    #[arg(long, value_name = "YYYY-MM-DD", value_parser = non_empty_trimmed)]
    to_date: Option<String>,
    /// Keywords for "Buscá por palabras clave", e.g. "impuesto".
    #[arg(long, value_parser = non_empty_trimmed)]
    query: Option<String>,
}

#[derive(Debug, Serialize, PartialEq)]
struct LawDetails {
    provincia: String,
    jurisdiccion: Jurisdiccion,
    titulo: String,
    ley: String,
    estado: Option<String>,
    url: String,
    pdf: String,
}

#[derive(Debug, Serialize, PartialEq)]
struct Normativa {
    provincia: Option<String>,
    jurisdiccion: Jurisdiccion,
    tipo_norma: String,
    titulo: String,
    // Issuing agency; only national rows carry it.
    #[serde(skip_serializing_if = "Option::is_none")]
    organismo: Option<String>,
    // Last path segment of the law url, e.g. "ley-11035-123456789-0abc-defg-373-0000svorpyel".
    ley: String,
    url: String,
    fecha_publicacion: Option<String>,
    descripcion: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Jurisdiccion {
    Provincial,
    Nacional,
}

impl Jurisdiccion {
    fn from_path_segment(segment: &str) -> Option<Self> {
        match segment {
            "provincial" => Some(Self::Provincial),
            "nacional" => Some(Self::Nacional),
            _ => None,
        }
    }

    fn path_segment(self) -> &'static str {
        match self {
            Self::Provincial => "provincial",
            Self::Nacional => "nacional",
        }
    }
}

/// `--law` ends up in a url path and a file name, so only slug characters are allowed.
fn validate_law_slug(ley: &str) -> Result<()> {
    if ley.is_empty() || !ley.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        bail!("invalid law `{ley}`: expected a slug of letters, digits and dashes");
    }
    Ok(())
}

fn validate_fetch(jurisdiccion: Jurisdiccion, ley: &str) -> Result<()> {
    match jurisdiccion {
        Jurisdiccion::Provincial => validate_law_slug(ley),
        Jurisdiccion::Nacional => bail!("fetching `nacional` laws is not supported yet"),
    }
}

/// `month` is 1..=12.
fn days_in_month(year: u32, month: u32) -> u32 {
    let is_leap_year =
        year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    match month {
        2 if is_leap_year => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// The site only accepts ISO dates in the url (`dd-mm-aaaa`, the form's placeholder, finds nothing).
fn validate_iso_date(flag: &str, value: &str) -> Result<()> {
    let parts: Vec<&str> = value.split('-').collect();
    let is_valid = value.chars().all(|c| c.is_ascii_digit() || c == '-')
        && match parts.as_slice() {
            [year, month, day] if year.len() == 4 && month.len() == 2 && day.len() == 2 => {
                match (
                    year.parse::<u32>(),
                    month.parse::<u32>(),
                    day.parse::<u32>(),
                ) {
                    (Ok(year), Ok(month @ 1..=12), Ok(day)) => {
                        (1..=days_in_month(year, month)).contains(&day)
                    }
                    _ => false,
                }
            }
            _ => false,
        };
    if !is_valid {
        bail!("invalid {flag} `{value}`: expected YYYY-MM-DD");
    }
    Ok(())
}

fn validate_national(filters: &NationalFilters) -> Result<()> {
    if *filters == NationalFilters::default() {
        // An unfiltered search is every norm (~200 pages at one page per crawl delay).
        bail!("pass at least one filter, e.g. --query or --law-type");
    }
    // The site answers this combination with a page that has no results block at all.
    if filters.law_type.as_deref() == Some("leyes") && filters.year.is_some() {
        bail!("--year finds no `leyes` on the site; use --from-date and --to-date instead");
    }
    if let Some(from_date) = &filters.from_date {
        validate_iso_date("--from-date", from_date)?;
    }
    if let Some(to_date) = &filters.to_date {
        validate_iso_date("--to-date", to_date)?;
    }
    if let (Some(from_date), Some(to_date)) = (&filters.from_date, &filters.to_date)
        && from_date > to_date
    {
        bail!("--from-date {from_date} is after --to-date {to_date}");
    }
    Ok(())
}

/// Splits a law link `/normativa/<jurisdiccion>/<ley>` into its jurisdiction and law slug.
fn parse_law_href(href: &str) -> Option<(Jurisdiccion, &str)> {
    let (segment, ley) = href
        .strip_prefix(SEARCH_PATH)?
        .strip_prefix('/')?
        .split_once('/')?;
    if ley.is_empty() || ley.contains('/') {
        return None;
    }
    Some((Jurisdiccion::from_path_segment(segment)?, ley))
}

#[derive(Debug, PartialEq)]
struct ResultsPage {
    total_pages: usize,
    rows: Vec<Normativa>,
}

fn selector(css: &str) -> Result<Selector> {
    Selector::parse(css).map_err(|error| anyhow!("invalid selector `{css}`: {error}"))
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Option values of the select matched by `select_css`, skipping the empty placeholder.
fn parse_options(html: &str, select_css: &str) -> Result<Vec<String>> {
    let option_selector = selector(&format!("{select_css} option"))?;
    Ok(Html::parse_document(html)
        .select(&option_selector)
        .filter_map(|option| option.value().attr("value"))
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect())
}

/// Pages in the results counter: provincial "<total> normas encontradas en <pages> páginas" has the
/// page count in its second `span`; national has it only as text ("... en 25 páginas").
fn parse_total_pages(document: &Html) -> Result<usize> {
    let provincial_counter = selector("div.m-b-2 span.fw-bold")?;
    let national_counter = selector("div.infoleg-search-results-count")?;

    let national_pages = document
        .select(&national_counter)
        .next()
        .and_then(|counter| {
            let text = collapse_whitespace(&counter.text().collect::<String>());
            let words: Vec<&str> = text.split(' ').collect();
            words
                .windows(3)
                .find(|window| window[0] == "en" && window[2].starts_with("página"))
                .and_then(|window| window[1].parse().ok())
        });
    let provincial_pages = || {
        document
            .select(&provincial_counter)
            .nth(1)
            .and_then(|span| {
                collapse_whitespace(&span.text().collect::<String>())
                    .parse()
                    .ok()
            })
    };
    // Absent counter means nothing matched.
    Ok(national_pages.or_else(provincial_pages).unwrap_or(0))
}

/// "Resolución GENERAL 5911/2026" -> "Resolución GENERAL": the words before the number.
fn norm_type_from_title(titulo: &str) -> String {
    let words: Vec<&str> = titulo
        .split_whitespace()
        .take_while(|word| !word.starts_with(|c: char| c.is_ascii_digit()))
        .collect();
    if words.is_empty() {
        titulo.to_owned()
    } else {
        words.join(" ")
    }
}

fn parse_results(html: &str, provincia: Option<&str>) -> Result<ResultsPage> {
    let document = Html::parse_document(html);
    // Provincial rows sit in `#normas`; national ones in the div right after the results counter.
    let row_selector = selector("#normas tbody tr, .infoleg-search-results-count ~ div tbody tr")?;
    let link_selector = selector(r#"td[data-label="Normativa"] a"#)?;
    let agency_selector = selector(r#"td[data-label="Normativa"] p.small"#)?;
    let time_selector = selector("time[datetime]")?;
    let description_selector = selector(r#"td[data-label="Descripción"] p.small"#)?;

    let total_pages = parse_total_pages(&document)?;

    let rows = document
        .select(&row_selector)
        .filter_map(|row| {
            let link = row.select(&link_selector).next()?;
            let href = link.value().attr("href")?;
            let Some((jurisdiccion, ley)) = parse_law_href(href) else {
                tracing::warn!(href, "unrecognised normativa link");
                return None;
            };
            let titulo = collapse_whitespace(&link.text().collect::<String>());
            Some(Normativa {
                provincia: provincia.map(str::to_owned),
                jurisdiccion,
                tipo_norma: match jurisdiccion {
                    Jurisdiccion::Provincial => TIPO_NORMA.to_owned(),
                    Jurisdiccion::Nacional => norm_type_from_title(&titulo),
                },
                titulo,
                organismo: row
                    .select(&agency_selector)
                    .map(|paragraph| collapse_whitespace(&paragraph.text().collect::<String>()))
                    .find(|text| !text.is_empty()),
                ley: ley.to_owned(),
                url: format!("{SITE_ORIGIN}{href}"),
                fecha_publicacion: row
                    .select(&time_selector)
                    .next()
                    .and_then(|time| time.value().attr("datetime"))
                    .map(str::to_owned),
                descripcion: row
                    .select(&description_selector)
                    .map(|paragraph| collapse_whitespace(&paragraph.text().collect::<String>()))
                    .filter(|text| !text.is_empty())
                    .collect(),
            })
        })
        .collect();

    Ok(ResultsPage { total_pages, rows })
}

/// Where the law's PDF goes: `--output` when given, otherwise `<ley>.pdf` in the current directory.
fn pdf_path(ley: &str, output: Option<&Path>) -> PathBuf {
    output.map_or_else(|| PathBuf::from(format!("{ley}.pdf")), Path::to_path_buf)
}

fn parse_law_page(
    html: &str,
    jurisdiccion: Jurisdiccion,
    ley: &str,
    pdf: &Path,
) -> Result<LawDetails> {
    let document = Html::parse_document(html);
    let text_of = |css: &str| -> Result<Option<String>> {
        Ok(document
            .select(&selector(css)?)
            .next()
            .map(|element| collapse_whitespace(&element.text().collect::<String>()))
            .filter(|text| !text.is_empty()))
    };
    let (Some(provincia), Some(titulo)) =
        (text_of(PROVINCE_SELECTOR)?, text_of(LAW_TITLE_SELECTOR)?)
    else {
        bail!("law not found: no province or title on the page; check `--law`");
    };
    Ok(LawDetails {
        provincia,
        jurisdiccion,
        titulo,
        ley: ley.to_owned(),
        estado: text_of(LAW_STATUS_SELECTOR)?,
        url: law_url(jurisdiccion, ley),
        pdf: pdf.display().to_string(),
    })
}

fn law_url(jurisdiccion: Jurisdiccion, ley: &str) -> String {
    format!(
        "{SITE_ORIGIN}{SEARCH_PATH}/{}/{ley}",
        jurisdiccion.path_segment()
    )
}

fn search_url(provincia: &str, query: &str, page_number: usize) -> Result<String> {
    let page_number = page_number.to_string();
    let url = Url::parse_with_params(
        &format!("{SITE_ORIGIN}{SEARCH_PATH}"),
        [
            ("provincia", provincia),
            ("jurisdiccion", "provincial"),
            ("tipo_norma", TIPO_NORMA),
            ("texto", query),
            ("limit", PAGE_SIZE),
            // The site's `offset` is a 1-based page number.
            ("offset", page_number.as_str()),
        ],
    )
    .context("failed to build search url")?;
    Ok(url.into())
}

/// National search url; the site wants every field present (empty when unused) and a 0-based `page`.
fn national_search_url(filters: &NationalFilters, page_index: usize) -> Result<String> {
    let law_number = filters.law_number.map(|number| number.to_string());
    let year = filters.year.map(|year| year.to_string());
    let page_index = page_index.to_string();
    let url = Url::parse_with_params(
        &format!("{SITE_ORIGIN}{SEARCH_PATH}"),
        [
            ("jurisdiccion", "nacional"),
            (
                "tipo_norma",
                filters.law_type.as_deref().unwrap_or_default(),
            ),
            ("numero", law_number.as_deref().unwrap_or_default()),
            ("anio", year.as_deref().unwrap_or_default()),
            ("dependencia", filters.agency.as_deref().unwrap_or_default()),
            (
                "publicacion_desde",
                filters.from_date.as_deref().unwrap_or_default(),
            ),
            (
                "publicacion_hasta",
                filters.to_date.as_deref().unwrap_or_default(),
            ),
            ("texto", filters.query.as_deref().unwrap_or_default()),
            ("s", "1"),
            ("page", page_index.as_str()),
        ],
    )
    .context("failed to build national search url")?;
    Ok(url.into())
}

async fn fetch_html(page: &Page, url: &str) -> Result<String> {
    page.goto(url)
        .await
        .with_context(|| format!("failed to open {url}"))?;
    page.content()
        .await
        .with_context(|| format!("failed to read content of {url}"))
}

async fn launch_browser() -> Result<(Browser, JoinHandle<()>, Page)> {
    let browser_config = BrowserConfig::builder()
        .build()
        .map_err(|error| anyhow!("invalid browser config: {error}"))?;
    let (browser, mut handler) = Browser::launch(browser_config)
        .await
        .context("failed to launch headless Chrome")?;
    // Chrome emits CDP messages chromiumoxide can't decode; those errors are not fatal, keep polling.
    let handler_task = tokio::spawn(async move { while handler.next().await.is_some() {} });
    let page = browser
        .new_page("about:blank")
        .await
        .context("failed to open browser page")?;
    Ok((browser, handler_task, page))
}

fn print_json_array<T: Serialize>(items: &[T], what: &str) -> Result<()> {
    writeln!(
        std::io::stdout(),
        "{}",
        serde_json::to_string_pretty(items)?
    )
    .with_context(|| format!("failed to write {what} to stdout"))
}

async fn run_list(page: &Page) -> Result<()> {
    let html = fetch_html(
        page,
        &format!("{SITE_ORIGIN}{SEARCH_PATH}?jurisdiccion=provincial"),
    )
    .await?;
    print_json_array(
        &parse_options(&html, "select[name=provincia]")?,
        "provinces",
    )
}

// The national page repeats some selects in a second form; only the main search form counts.
const NATIONAL_FORM_SELECTOR: &str = "form#infoleg-normativa-search-form";

fn parse_agencies(html: &str) -> Result<Vec<String>> {
    parse_options(
        html,
        &format!("{NATIONAL_FORM_SELECTOR} select[name=dependencia]"),
    )
}

fn parse_years(html: &str) -> Result<Vec<u16>> {
    parse_options(html, &format!("{NATIONAL_FORM_SELECTOR} select[name=anio]"))?
        .iter()
        .map(|year| {
            year.parse()
                .with_context(|| format!("year option `{year}` is not a number"))
        })
        .collect()
}

async fn fetch_national_form(page: &Page) -> Result<String> {
    fetch_html(
        page,
        &format!("{SITE_ORIGIN}{SEARCH_PATH}?jurisdiccion=nacional"),
    )
    .await
}

/// Lists of the national search form. `LawType` is a local constant (`main` answers it before
/// launching Chrome); it is kept here so the match stays exhaustive.
async fn run_list_national(page: &Page, what: NationalList) -> Result<()> {
    match what {
        NationalList::Agencies => print_json_array(
            &parse_agencies(&fetch_national_form(page).await?)?,
            "agencies",
        ),
        NationalList::Years => {
            print_json_array(&parse_years(&fetch_national_form(page).await?)?, "years")
        }
        NationalList::LawType => print_json_array(LAW_TYPES, "law types"),
    }
}

/// Pages to walk: all of them, unless the cap is lower.
fn pages_to_fetch(total_pages: usize, max_pages: usize) -> usize {
    total_pages.min(max_pages)
}

/// Walks the results pages of a search (at most `max_pages`), printing each law as a JSON line;
/// returns the row count. `url_for_page` takes a 0-based page index. The total page count comes
/// from the first page.
async fn run_search(
    page: &Page,
    provincia: Option<&str>,
    max_pages: usize,
    url_for_page: impl Fn(usize) -> Result<String>,
) -> Result<usize> {
    let mut page_index = 0;
    let mut last_page = 1;
    let mut written = 0usize;
    while page_index < last_page {
        if page_index > 0 {
            tokio::time::sleep(CRAWL_DELAY).await;
        }
        let html = fetch_html(page, &url_for_page(page_index)?).await?;
        let results = parse_results(&html, provincia)?;
        let total_pages = results.total_pages;
        if page_index == 0 {
            last_page = pages_to_fetch(total_pages, max_pages);
            if last_page < total_pages {
                tracing::warn!(
                    fetching = last_page,
                    total_pages,
                    "result truncated; raise --max-pages to fetch more"
                );
            }
        }
        for normativa in &results.rows {
            writeln!(std::io::stdout(), "{}", serde_json::to_string(normativa)?)
                .context("failed to write normativa to stdout")?;
            written += 1;
        }
        tracing::info!(
            provincia,
            page_number = page_index + 1,
            total_pages,
            "page fetched"
        );
        page_index += 1;
    }
    Ok(written)
}

async fn run_query(page: &Page, provincia: &str, query: &str) -> Result<()> {
    // The provincial site's `offset` is a 1-based page number.
    let written = run_search(page, Some(provincia), usize::MAX, |page_index| {
        search_url(provincia, query, page_index + 1)
    })
    .await?;
    if written == 0 {
        tracing::warn!(
            provincia,
            query,
            "no results; check the province name with `list`"
        );
    }
    Ok(())
}

async fn run_query_national(page: &Page, args: &NationalArgs) -> Result<()> {
    let max_pages = usize::try_from(args.max_pages).unwrap_or(usize::MAX);
    let written = run_search(page, None, max_pages, |page_index| {
        national_search_url(&args.filters, page_index)
    })
    .await?;
    if written == 0 {
        tracing::warn!("no results; check the filters (agency names must match exactly)");
    }
    Ok(())
}

// The click only starts the navigation, so poll until the page url shows it happened.
async fn wait_for_url_suffix(page: &Page, suffix: &str) -> Result<()> {
    const POLL_INTERVAL: Duration = Duration::from_millis(250);
    const MAX_POLLS: u32 = 60;
    for _ in 0..MAX_POLLS {
        let current = page.url().await.context("failed to read page url")?;
        if current.as_deref().is_some_and(|url| url.ends_with(suffix)) {
            return Ok(());
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    bail!("page url did not end with `{suffix}` after {MAX_POLLS} polls")
}

async fn run_fetch(
    page: &Page,
    jurisdiccion: Jurisdiccion,
    ley: &str,
    output: Option<&Path>,
) -> Result<()> {
    let pdf = pdf_path(ley, output);
    let html = fetch_html(page, &law_url(jurisdiccion, ley)).await?;
    let details = parse_law_page(&html, jurisdiccion, ley, &pdf)?;

    // A second request follows, so honour the crawl delay.
    tokio::time::sleep(CRAWL_DELAY).await;
    page.find_element(VIEW_LAW_BUTTON_SELECTOR)
        .await
        .context("failed to find the \"Ver norma\" button")?
        .click()
        .await
        .context("failed to click the \"Ver norma\" button")?;
    wait_for_url_suffix(page, VIEW_LAW_PATH_SUFFIX)
        .await
        .context("clicking \"Ver norma\" did not open the law text page")?;
    page.wait_for_navigation()
        .await
        .context("failed to load the law text page")?;

    let pdf_params = PrintToPdfParams {
        print_background: Some(true),
        ..Default::default()
    };
    page.save_pdf(pdf_params, &pdf)
        .await
        .with_context(|| format!("failed to save {}", pdf.display()))?;

    writeln!(std::io::stdout(), "{}", serde_json::to_string(&details)?)
        .context("failed to write law details to stdout")
}

/// Runs one parsed command: validates input, drives Chrome and prints the result to stdout.
///
/// Validation happens before Chrome starts so `--help` and bad arguments never launch a browser.
pub async fn run(cli: Cli) -> Result<()> {
    match &cli.command {
        Command::Fetch {
            jurisdiction, law, ..
        } => validate_fetch(*jurisdiction, law)?,
        Command::Query {
            scope: Some(QueryScope::National(args)),
            ..
        } => validate_national(&args.filters)?,
        Command::Query {
            scope: None,
            province,
            query,
        } if province.is_none() || query.is_none() => {
            bail!("--province and --query are required")
        }
        // Slugs are a local constant: answer without launching Chrome.
        Command::List {
            scope:
                Some(ListScope::National {
                    what: NationalList::LawType,
                }),
        } => return print_json_array(LAW_TYPES, "law types"),
        Command::List { .. } | Command::Query { .. } => {}
    }

    let (mut browser, handler_task, page) = launch_browser().await?;
    let outcome = match &cli.command {
        Command::List { scope: None } => run_list(&page).await,
        Command::List {
            scope: Some(ListScope::National { what }),
        } => run_list_national(&page, *what).await,
        Command::Query {
            scope: Some(QueryScope::National(args)),
            ..
        } => run_query_national(&page, args).await,
        Command::Query {
            province: Some(province),
            query: Some(query),
            ..
        } => run_query(&page, province, query).await,
        // Unreachable: clap requires both flags unless a subcommand is given, and the check above
        // runs before launching Chrome.
        Command::Query { .. } => bail!("--province and --query are required"),
        Command::Fetch {
            jurisdiction,
            law,
            output,
        } => run_fetch(&page, *jurisdiction, law, output.as_deref()).await,
    };
    // Close before propagating the command error so Chrome never outlives us.
    browser.close().await.context("failed to close browser")?;
    handler_task
        .await
        .context("browser handler task panicked")?;
    outcome
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    const RESULTS_FIXTURE: &str = include_str!("../tests/fixtures/results.html");
    const NATIONAL_RESULTS_FIXTURE: &str = include_str!("../tests/fixtures/results_national.html");
    const LAW_FIXTURE: &str = include_str!("../tests/fixtures/law.html");
    const LAW_SLUG: &str = "ley-14709-123456789-0abc-defg-907-4100bvorpyel";

    #[test]
    fn parse_results_buenos_aires_laws_page_returns_fifty_rows_over_three_pages() -> Result<()> {
        // Arrange / Act
        let results = parse_results(RESULTS_FIXTURE, Some("Buenos Aires"))?;

        // Assert
        assert_eq!(results.total_pages, 3);
        assert_eq!(results.rows.len(), 50);
        let first = &results.rows[0];
        assert_eq!(first.provincia.as_deref(), Some("Buenos Aires"));
        assert_eq!(first.jurisdiccion, Jurisdiccion::Provincial);
        assert_eq!(first.tipo_norma, "Ley");
        assert!(first.ley.starts_with("ley-"));
        assert_eq!(
            first.url,
            format!("{SITE_ORIGIN}/normativa/provincial/{}", first.ley)
        );
        assert!(!first.titulo.is_empty());
        assert!(first.fecha_publicacion.is_some());
        Ok(())
    }

    #[test]
    fn parse_results_no_results_page_returns_empty() -> Result<()> {
        // Arrange
        let html = "<html><body><p>No se encontraron normas</p></body></html>";

        // Act
        let results = parse_results(html, Some("Chaco"))?;

        // Assert
        assert_eq!(
            results,
            ResultsPage {
                total_pages: 0,
                rows: vec![]
            }
        );
        Ok(())
    }

    #[test]
    fn parse_results_without_province_serialises_null_province() -> Result<()> {
        // Arrange
        let html = r#"<div id="normas"><table><tbody><tr>
            <td data-label="Normativa"><a href="/normativa/nacional/ley-1-abc"> Ley 1 </a></td>
            </tr></tbody></table></div>"#;

        // Act
        let results = parse_results(html, None)?;
        let json = serde_json::to_string(&results.rows[0])?;

        // Assert
        assert!(json.contains(r#""provincia":null"#));
        assert!(json.contains(r#""jurisdiccion":"nacional""#));
        assert!(json.contains(r#""ley":"ley-1-abc""#));
        Ok(())
    }

    #[test]
    fn parse_results_national_fixture_returns_five_rows_over_212_pages() -> Result<()> {
        // Act
        let results = parse_results(NATIONAL_RESULTS_FIXTURE, None)?;

        // Assert
        assert_eq!(results.total_pages, 212);
        assert_eq!(results.rows.len(), 5);
        let first = &results.rows[0];
        assert_eq!(first.provincia, None);
        assert_eq!(first.jurisdiccion, Jurisdiccion::Nacional);
        assert_eq!(first.tipo_norma, "Ley");
        assert_eq!(first.titulo, "Ley 27826");
        assert_eq!(first.ley, "norma-431078");
        assert_eq!(
            first.url,
            format!("{SITE_ORIGIN}/normativa/nacional/norma-431078")
        );
        assert_eq!(
            first.organismo.as_deref(),
            Some("HONORABLE CONGRESO DE LA NACION ARGENTINA")
        );
        assert_eq!(first.fecha_publicacion.as_deref(), Some("2026-10-09"));
        assert_eq!(first.descripcion.len(), 3);
        assert_eq!(results.rows[1].tipo_norma, "Resolución GENERAL");
        Ok(())
    }

    #[test]
    fn parse_results_national_zero_results_page_returns_empty() -> Result<()> {
        // Arrange
        let html = r#"<div class="infoleg-search-results-count m-b-2">
            <span class="fw-semibold">0</span> norma encontrada</div>
            <table><tbody><tr><td colspan="3">No se encontraron resultados</td></tr></tbody></table>"#;

        // Act
        let results = parse_results(html, None)?;

        // Assert
        assert_eq!(
            results,
            ResultsPage {
                total_pages: 0,
                rows: vec![]
            }
        );
        Ok(())
    }

    #[test]
    fn parse_results_provincial_fixture_rows_are_all_provincial_without_organismo() -> Result<()> {
        // Act
        let results = parse_results(RESULTS_FIXTURE, Some("Buenos Aires"))?;

        // Assert
        assert_eq!(results.rows.len(), 50);
        for row in &results.rows {
            assert_eq!(row.jurisdiccion, Jurisdiccion::Provincial);
            assert_eq!(row.tipo_norma, "Ley");
            assert!(!serde_json::to_string(row)?.contains("organismo"));
        }
        Ok(())
    }

    #[test]
    fn parse_results_ignores_rows_of_unrelated_tables() -> Result<()> {
        // Arrange
        let html = r#"<table><tbody><tr>
            <td data-label="Normativa"><a href="/normativa/nacional/norma-1">Ley 1</a></td>
            </tr></tbody></table>"#;

        // Act
        let results = parse_results(html, None)?;

        // Assert
        assert_eq!(results.rows, vec![]);
        Ok(())
    }

    #[test]
    fn parse_results_national_single_result_page_counts_one_page() -> Result<()> {
        // Arrange
        let html = r#"<div class="infoleg-search-results-count m-b-2">
            <span class="fw-semibold">1</span> norma encontrada en 1 página</div>
            <div class=""><table><tbody><tr>
            <td data-label="Normativa"><a href="/normativa/nacional/norma-1">Ley 1</a></td>
            </tr></tbody></table></div>"#;

        // Act
        let results = parse_results(html, None)?;

        // Assert
        assert_eq!(results.total_pages, 1);
        assert_eq!(results.rows.len(), 1);
        Ok(())
    }

    #[test]
    fn norm_type_from_title_drops_number_and_year() {
        // Act / Assert
        assert_eq!(
            norm_type_from_title("Resolución GENERAL 5911/2026"),
            "Resolución GENERAL"
        );
        assert_eq!(
            norm_type_from_title("Decisión Administrativa 3/2024"),
            "Decisión Administrativa"
        );
        assert_eq!(norm_type_from_title("27826"), "27826");
    }

    #[test]
    fn parse_law_href_provincial_link_returns_jurisdiction_and_slug() {
        // Act
        let parsed =
            parse_law_href("/normativa/provincial/ley-11035-123456789-0abc-defg-373-0000svorpyel");

        // Assert
        assert_eq!(
            parsed,
            Some((
                Jurisdiccion::Provincial,
                "ley-11035-123456789-0abc-defg-373-0000svorpyel"
            ))
        );
    }

    #[test]
    fn parse_law_href_nacional_link_returns_nacional() {
        // Act
        let parsed = parse_law_href("/normativa/nacional/ley-27078-abc");

        // Assert
        assert_eq!(parsed, Some((Jurisdiccion::Nacional, "ley-27078-abc")));
    }

    #[test]
    fn parse_law_href_unknown_jurisdiction_returns_none() {
        // Act / Assert
        assert_eq!(parse_law_href("/normativa/municipal/ley-1"), None);
    }

    #[test]
    fn parse_law_href_missing_slug_returns_none() {
        // Act / Assert
        assert_eq!(parse_law_href("/normativa/provincial/"), None);
        assert_eq!(parse_law_href("/normativa/provincial"), None);
        assert_eq!(parse_law_href("/otra/provincial/ley-1"), None);
    }

    #[test]
    fn parse_options_select_with_placeholder_skips_empty_value() -> Result<()> {
        // Arrange
        let html = r#"<select name="provincia"><option value="">-</option>
            <option value="Córdoba">Córdoba</option></select>"#;

        // Act
        let options = parse_options(html, "select[name=provincia]")?;

        // Assert
        assert_eq!(options, vec!["Córdoba".to_owned()]);
        Ok(())
    }

    #[test]
    fn search_url_accented_province_is_percent_encoded() -> Result<()> {
        // Act
        let url = search_url("Río Negro", "impuesto tasa", 2)?;

        // Assert
        assert_eq!(
            url,
            "https://www.argentina.gob.ar/normativa?provincia=R%C3%ADo+Negro&jurisdiccion=provincial\
             &tipo_norma=Ley&texto=impuesto+tasa&limit=50&offset=2"
        );
        Ok(())
    }

    #[test]
    fn cli_list_without_arguments_parses() {
        // Act
        let cli = Cli::try_parse_from(["argentina-normativa-cli", "list"]);

        // Assert
        assert!(matches!(
            cli,
            Ok(Cli {
                command: Command::List { scope: None }
            })
        ));
    }

    #[test]
    fn cli_list_national_subcommands_parse() {
        // Arrange
        let cases = [
            ("agencies", NationalList::Agencies),
            ("law-type", NationalList::LawType),
            ("years", NationalList::Years),
        ];

        for (name, expected) in cases {
            // Act
            let cli = Cli::try_parse_from(["argentina-normativa-cli", "list", "national", name]);

            // Assert
            assert!(
                matches!(
                    cli,
                    Ok(Cli {
                        command: Command::List {
                            scope: Some(ListScope::National { what })
                        }
                    }) if what == expected
                ),
                "{name}"
            );
        }
    }

    #[test]
    fn cli_list_national_without_or_with_unknown_list_fails() {
        // Act
        let missing = Cli::try_parse_from(["argentina-normativa-cli", "list", "national"]);
        let unknown = Cli::try_parse_from(["argentina-normativa-cli", "list", "national", "bogus"]);

        // Assert
        assert!(missing.is_err());
        assert!(unknown.is_err());
    }

    #[test]
    fn parse_agencies_national_fixture_ignores_placeholder_and_second_form() -> Result<()> {
        // Arrange
        let html = std::fs::read_to_string("tests/fixtures/form_national.html")?;

        // Act
        let agencies = parse_agencies(&html)?;

        // Assert
        assert_eq!(
            agencies,
            vec![
                "1RA. COM. NAC. DE SALARIOS IND. DEL VESTIDO TRABAJO A DOMIC.".to_owned(),
                "ADM. GRAL. DEL SERVICIO NACIONAL DE SANIDAD ANIMAL".to_owned(),
                "ADMINISTRACION ADUANA BARILOCHE".to_owned(),
            ]
        );
        Ok(())
    }

    #[test]
    fn parse_years_national_fixture_keeps_site_order_as_numbers() -> Result<()> {
        // Arrange
        let html = std::fs::read_to_string("tests/fixtures/form_national.html")?;

        // Act
        let years = parse_years(&html)?;

        // Assert
        assert_eq!(years, vec![2026, 2025, 2024]);
        Ok(())
    }

    #[test]
    fn parse_years_non_numeric_option_returns_error() {
        // Arrange
        let html = r#"<form id="infoleg-normativa-search-form"><select name="anio">
            <option value="dos mil">x</option></select></form>"#;

        // Act
        let result = parse_years(html);

        // Assert
        assert!(result.is_err());
    }

    #[test]
    fn cli_query_with_flags_parses_province_and_query() {
        // Act
        let cli = Cli::try_parse_from([
            "argentina-normativa-cli",
            "query",
            "--province",
            "Córdoba",
            "--query",
            "impuesto tasa",
        ]);

        // Assert
        assert!(matches!(
            cli,
            Ok(Cli { command: Command::Query { province: Some(province), query: Some(query), scope: None } })
                if province == "Córdoba" && query == "impuesto tasa"
        ));
    }

    #[test]
    fn cli_query_without_province_fails() {
        // Act
        let cli = Cli::try_parse_from(["argentina-normativa-cli", "query", "--query", "impuesto"]);

        // Assert
        assert!(cli.is_err());
    }

    #[test]
    fn national_search_url_full_filters_are_encoded_in_site_order() -> Result<()> {
        // Arrange
        let filters = NationalFilters {
            law_type: Some("decretos".to_owned()),
            law_number: Some(70),
            year: Some(2023),
            agency: Some("MINISTERIO DE ECONOMIA".to_owned()),
            from_date: Some("2023-01-01".to_owned()),
            to_date: Some("2023-12-31".to_owned()),
            query: Some("impuesto tasa".to_owned()),
        };

        // Act
        let url = national_search_url(&filters, 2)?;

        // Assert
        assert_eq!(
            url,
            "https://www.argentina.gob.ar/normativa?jurisdiccion=nacional&tipo_norma=decretos\
             &numero=70&anio=2023&dependencia=MINISTERIO+DE+ECONOMIA\
             &publicacion_desde=2023-01-01&publicacion_hasta=2023-12-31\
             &texto=impuesto+tasa&s=1&page=2"
        );
        Ok(())
    }

    #[test]
    fn national_search_url_only_query_keeps_empty_fields() -> Result<()> {
        // Arrange
        let filters = NationalFilters {
            query: Some("impuesto".to_owned()),
            ..Default::default()
        };

        // Act
        let url = national_search_url(&filters, 0)?;

        // Assert
        assert_eq!(
            url,
            "https://www.argentina.gob.ar/normativa?jurisdiccion=nacional&tipo_norma=&numero=\
             &anio=&dependencia=&publicacion_desde=&publicacion_hasta=&texto=impuesto&s=1&page=0"
        );
        Ok(())
    }

    #[test]
    fn validate_national_no_filters_is_rejected() {
        // Act
        let result = validate_national(&NationalFilters::default());

        // Assert
        assert!(result.is_err_and(|error| error.to_string().contains("at least one filter")));
    }

    #[test]
    fn validate_national_leyes_with_year_is_rejected() {
        // Arrange
        let filters = NationalFilters {
            law_type: Some("leyes".to_owned()),
            year: Some(2024),
            ..Default::default()
        };

        // Act / Assert
        assert!(validate_national(&filters).is_err());
    }

    #[test]
    fn validate_national_bad_dates_are_rejected() {
        // Act / Assert
        for invalid in [
            "01-01-2024",
            "2024-13-01",
            "2024-1-1",
            "2024-01-+1",
            "",
            "hoy",
        ] {
            let filters = NationalFilters {
                from_date: Some(invalid.to_owned()),
                ..Default::default()
            };
            assert!(validate_national(&filters).is_err(), "accepted `{invalid}`");
        }
    }

    #[test]
    fn validate_national_impossible_calendar_dates_are_rejected() {
        // Act / Assert
        for invalid in [
            "2024-02-30",
            "2023-02-29",
            "2024-04-31",
            "1900-02-29",
            "2024-00-10",
            "2024-06-00",
        ] {
            let filters = NationalFilters {
                to_date: Some(invalid.to_owned()),
                ..Default::default()
            };
            assert!(validate_national(&filters).is_err(), "accepted `{invalid}`");
        }
    }

    #[test]
    fn validate_national_leap_days_are_accepted() {
        // Act / Assert
        for valid in ["2024-02-29", "2000-02-29", "2023-02-28", "2024-12-31"] {
            let filters = NationalFilters {
                to_date: Some(valid.to_owned()),
                ..Default::default()
            };
            assert!(validate_national(&filters).is_ok(), "rejected `{valid}`");
        }
    }

    #[test]
    fn validate_national_reversed_date_range_is_rejected() {
        // Arrange
        let filters = NationalFilters {
            from_date: Some("2024-02-01".to_owned()),
            to_date: Some("2024-01-01".to_owned()),
            ..Default::default()
        };

        // Act
        let result = validate_national(&filters);

        // Assert
        assert!(result.is_err_and(|error| error.to_string().contains("after --to-date")));
    }

    #[test]
    fn validate_national_valid_filters_are_accepted() {
        // Arrange
        let filters = NationalFilters {
            law_type: Some("decretos".to_owned()),
            year: Some(2024),
            from_date: Some("2024-01-01".to_owned()),
            to_date: Some("2024-01-31".to_owned()),
            ..Default::default()
        };

        // Act / Assert
        assert!(validate_national(&filters).is_ok());
    }

    #[test]
    fn cli_query_national_with_flags_parses_every_filter() {
        // Act
        let cli = Cli::try_parse_from([
            "argentina-normativa-cli",
            "query",
            "national",
            "--law-type",
            "resoluciones",
            "--law-number",
            "5911",
            "--year",
            "2026",
            "--agency",
            "AGENCIA DE RECAUDACION Y CONTROL ADUANERO",
            "--from-date",
            "2026-01-01",
            "--to-date",
            "2026-12-31",
            "--query",
            "impuesto",
        ]);

        // Assert
        assert!(matches!(
            cli,
            Ok(Cli { command: Command::Query { province: None, query: None, scope: Some(QueryScope::National(args)) } })
                if args.max_pages == DEFAULT_MAX_PAGES && args.filters == NationalFilters {
                    law_type: Some("resoluciones".to_owned()),
                    law_number: Some(5911),
                    year: Some(2026),
                    agency: Some("AGENCIA DE RECAUDACION Y CONTROL ADUANERO".to_owned()),
                    from_date: Some("2026-01-01".to_owned()),
                    to_date: Some("2026-12-31".to_owned()),
                    query: Some("impuesto".to_owned()),
                }
        ));
    }

    #[test]
    fn cli_query_national_empty_or_blank_values_fail() {
        // Act / Assert
        for flag in ["--query", "--agency", "--from-date", "--to-date"] {
            for blank in ["", "   "] {
                let cli = Cli::try_parse_from([
                    "argentina-normativa-cli",
                    "query",
                    "national",
                    flag,
                    blank,
                ]);
                assert!(cli.is_err(), "accepted `{flag} {blank:?}`");
            }
        }
    }

    #[test]
    fn cli_query_national_padded_values_are_trimmed() {
        // Act
        let cli = Cli::try_parse_from([
            "argentina-normativa-cli",
            "query",
            "national",
            "--query",
            "  impuesto ",
            "--agency",
            " MINISTERIO DE ECONOMIA",
        ]);

        // Assert
        assert!(matches!(
            cli,
            Ok(Cli { command: Command::Query { scope: Some(QueryScope::National(args)), .. } })
                if args.filters.query.as_deref() == Some("impuesto")
                    && args.filters.agency.as_deref() == Some("MINISTERIO DE ECONOMIA")
        ));
    }

    #[test]
    fn cli_query_national_max_pages_flag_overrides_default() {
        // Act
        let cli = Cli::try_parse_from([
            "argentina-normativa-cli",
            "query",
            "national",
            "--query",
            "impuesto",
            "--max-pages",
            "3",
        ]);

        // Assert
        assert!(matches!(
            cli,
            Ok(Cli { command: Command::Query { scope: Some(QueryScope::National(args)), .. } })
                if args.max_pages == 3
        ));
    }

    #[test]
    fn cli_query_national_zero_max_pages_fails() {
        // Act
        let cli = Cli::try_parse_from([
            "argentina-normativa-cli",
            "query",
            "national",
            "--query",
            "impuesto",
            "--max-pages",
            "0",
        ]);

        // Assert
        assert!(cli.is_err());
    }

    #[test]
    fn pages_to_fetch_caps_total_pages() {
        // Act / Assert
        assert_eq!(pages_to_fetch(212, 20), 20);
        assert_eq!(pages_to_fetch(3, 20), 3);
        assert_eq!(pages_to_fetch(0, 20), 0);
        assert_eq!(pages_to_fetch(5, usize::MAX), 5);
    }

    #[test]
    fn cli_query_national_unknown_law_type_fails() {
        // Act
        let cli = Cli::try_parse_from([
            "argentina-normativa-cli",
            "query",
            "national",
            "--law-type",
            "ordenanzas",
        ]);

        // Assert
        assert!(cli.is_err());
    }

    #[test]
    fn cli_query_national_non_numeric_law_number_fails() {
        // Act
        let cli = Cli::try_parse_from([
            "argentina-normativa-cli",
            "query",
            "national",
            "--law-number",
            "12a",
        ]);

        // Assert
        assert!(cli.is_err());
    }

    #[test]
    fn cli_query_province_flags_with_national_subcommand_fail() {
        // Act
        let cli = Cli::try_parse_from([
            "argentina-normativa-cli",
            "query",
            "--province",
            "Córdoba",
            "national",
            "--query",
            "impuesto",
        ]);

        // Assert
        assert!(cli.is_err());
    }

    #[test]
    fn parse_law_page_buenos_aires_fixture_returns_province_title_status() -> Result<()> {
        // Act
        let details = parse_law_page(
            LAW_FIXTURE,
            Jurisdiccion::Provincial,
            LAW_SLUG,
            Path::new("out/ley.pdf"),
        )?;

        // Assert
        assert_eq!(
            details,
            LawDetails {
                provincia: "Buenos Aires".to_owned(),
                jurisdiccion: Jurisdiccion::Provincial,
                titulo: "Ley 14709".to_owned(),
                ley: LAW_SLUG.to_owned(),
                estado: Some("Vigente, de alcance general".to_owned()),
                url: format!("{SITE_ORIGIN}/normativa/provincial/{LAW_SLUG}"),
                pdf: "out/ley.pdf".to_owned(),
            }
        );
        Ok(())
    }

    #[test]
    fn parse_law_page_page_without_label_returns_not_found_error() {
        // Arrange
        let html = "<html><body><h1>Página no encontrada</h1></body></html>";

        // Act
        let result = parse_law_page(
            html,
            Jurisdiccion::Provincial,
            "ley-1",
            Path::new("ley-1.pdf"),
        );

        // Assert
        assert!(result.is_err_and(|error| error.to_string().contains("law not found")));
    }

    #[test]
    fn validate_law_slug_real_slug_is_accepted() {
        // Act / Assert
        assert!(validate_law_slug(LAW_SLUG).is_ok());
    }

    #[test]
    fn validate_law_slug_path_like_or_empty_values_are_rejected() {
        // Act / Assert
        for invalid in ["", "../x", "a/b", "ley 1", "ley-1.pdf", "ley\u{301}"] {
            assert!(validate_law_slug(invalid).is_err(), "accepted `{invalid}`");
        }
    }

    #[test]
    fn validate_fetch_nacional_returns_unsupported_error() {
        // Act
        let result = validate_fetch(Jurisdiccion::Nacional, "ley-1");

        // Assert
        assert!(result.is_err_and(|error| error.to_string().contains("not supported yet")));
    }

    #[test]
    fn cli_fetch_with_flags_parses_jurisdiction_and_law() {
        // Act
        let cli = Cli::try_parse_from([
            "argentina-normativa-cli",
            "fetch",
            "--jurisdiction",
            "provincial",
            "--law",
            LAW_SLUG,
        ]);

        // Assert
        assert!(matches!(
            cli,
            Ok(Cli { command: Command::Fetch { jurisdiction: Jurisdiccion::Provincial, law, output: None } })
                if law == LAW_SLUG
        ));
    }

    #[test]
    fn cli_fetch_unknown_jurisdiction_fails() {
        // Act
        let cli = Cli::try_parse_from([
            "argentina-normativa-cli",
            "fetch",
            "--jurisdiction",
            "municipal",
            "--law",
            "ley-1",
        ]);

        // Assert
        assert!(cli.is_err());
    }

    #[test]
    fn cli_fetch_without_law_fails() {
        // Act
        let cli = Cli::try_parse_from([
            "argentina-normativa-cli",
            "fetch",
            "--jurisdiction",
            "provincial",
        ]);

        // Assert
        assert!(cli.is_err());
    }

    #[test]
    fn cli_fetch_with_output_flag_parses_file_path() {
        // Act
        let cli = Cli::try_parse_from([
            "argentina-normativa-cli",
            "fetch",
            "--jurisdiction",
            "provincial",
            "--law",
            "ley-1",
            "--output",
            "laws/ley-1.pdf",
        ]);

        // Assert
        assert!(matches!(
            cli,
            Ok(Cli { command: Command::Fetch { output: Some(output), .. } })
                if output == Path::new("laws/ley-1.pdf")
        ));
    }

    #[test]
    fn pdf_path_without_output_defaults_to_law_slug() {
        // Act
        let path = pdf_path(LAW_SLUG, None);

        // Assert
        assert_eq!(path, PathBuf::from(format!("{LAW_SLUG}.pdf")));
    }

    #[test]
    fn pdf_path_with_output_uses_given_path() {
        // Act
        let path = pdf_path(LAW_SLUG, Some(Path::new("laws/custom.pdf")));

        // Assert
        assert_eq!(path, PathBuf::from("laws/custom.pdf"));
    }
}
