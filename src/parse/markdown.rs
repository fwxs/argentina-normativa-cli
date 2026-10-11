//! Renders the HTML blocks of a law text as Markdown.
//!
//! Block elements (paragraphs, headings, lists, tables, ...) become separate Markdown blocks, so
//! neighbouring blocks are never run together. Text of any other element is kept inline.

use scraper::{ElementRef, Node};

use super::collapse_whitespace;

// Stands for a `<br>` in inline text. It is not whitespace, so trimming around emphasis keeps it; two
// of them in a row end a paragraph (see `finish_paragraphs`), a lone one reads as a space.
const LINE_BREAK: char = '\u{E000}';

#[derive(Clone, Copy)]
enum Block {
    /// Only groups other blocks: its children are rendered in place.
    Container,
    Paragraph,
    Heading(usize),
    List {
        ordered: bool,
    },
    Table,
    Quote,
    Preformatted,
    Rule,
}

#[derive(Clone, Copy, Default)]
struct Emphasis {
    bold: bool,
    italic: bool,
}

fn block_kind(tag: &str) -> Option<Block> {
    match tag {
        "div" | "section" | "article" | "aside" | "header" | "footer" | "main" | "nav"
        | "figure" | "center" | "form" | "fieldset" | "dl" => Some(Block::Container),
        "p" | "li" | "dt" | "dd" | "figcaption" | "caption" | "address" | "summary" | "legend" => {
            Some(Block::Paragraph)
        }
        "h1" => Some(Block::Heading(1)),
        "h2" => Some(Block::Heading(2)),
        "h3" => Some(Block::Heading(3)),
        "h4" => Some(Block::Heading(4)),
        "h5" => Some(Block::Heading(5)),
        "h6" => Some(Block::Heading(6)),
        "ul" => Some(Block::List { ordered: false }),
        "ol" => Some(Block::List { ordered: true }),
        "table" => Some(Block::Table),
        "blockquote" => Some(Block::Quote),
        "pre" => Some(Block::Preformatted),
        "hr" => Some(Block::Rule),
        _ => None,
    }
}

/// Screen-reader-only headings and non-content elements are not part of the law text.
fn is_hidden(element: ElementRef) -> bool {
    matches!(
        element.value().name(),
        "script" | "style" | "template" | "noscript"
    ) || element.value().classes().any(|class| class == "sr-only")
}

/// Escapes the characters Markdown would read as inline markup and drops the line-break stand-in.
pub(super) fn escape_inline(text: &str) -> String {
    text.chars().fold(
        String::with_capacity(text.len()),
        |mut escaped, character| {
            // Page text must never carry the `<br>` stand-in, or it would be read as a line break.
            if character == LINE_BREAK {
                return escaped;
            }
            if matches!(character, '\\' | '*' | '_' | '`' | '[' | ']' | '<') {
                escaped.push('\\');
            }
            escaped.push(character);
            escaped
        },
    )
}

/// Escapes a leading `#`, `>`, `-`, `+` or `1.`, which Markdown would read as block markup.
fn escape_block_start(text: &str) -> String {
    if text.starts_with(['#', '>', '-', '+']) {
        return format!("\\{text}");
    }
    let digits = text.chars().take_while(char::is_ascii_digit).count();
    let rest = &text[digits..];
    let is_list_marker = digits > 0
        && rest.starts_with(['.', ')'])
        && (rest.len() == 1 || rest[1..].starts_with(' '));
    if is_list_marker {
        return format!("{}\\{rest}", &text[..digits]);
    }
    text.to_owned()
}

/// Inline text on a single line: every line break becomes a space.
fn flatten_breaks(text: &str) -> String {
    text.replace(LINE_BREAK, " ")
}

/// A finished one-line block: whitespace collapsed and block-level markup escaped.
fn finish_line(text: &str) -> Option<String> {
    Some(escape_block_start(&collapse_whitespace(&flatten_breaks(
        text,
    ))))
    .filter(|line| !line.is_empty())
}

/// The finished blocks of inline text: two or more line breaks in a row separate paragraphs.
fn finish_paragraphs(text: &str) -> Vec<String> {
    let mut paragraphs = Vec::new();
    let mut current = String::new();
    for segment in text.split(LINE_BREAK) {
        if segment.trim().is_empty() && !current.is_empty() {
            paragraphs.extend(finish_line(&std::mem::take(&mut current)));
        }
        current.push_str(segment);
        current.push(' ');
    }
    paragraphs.extend(finish_line(&current));
    paragraphs
}

/// Splits `text` into the single space (if any) before it, the text itself and the space after.
fn edge_spaces(text: &str) -> (&'static str, &str, &'static str) {
    let core = text.trim();
    if core.is_empty() {
        return ("", core, "");
    }
    let leading = if text.starts_with(char::is_whitespace) {
        " "
    } else {
        ""
    };
    let trailing = if text.ends_with(char::is_whitespace) {
        " "
    } else {
        ""
    };
    (leading, core, trailing)
}

/// Emphasis markers must hug the text, so the edge spaces go outside them.
fn wrap_emphasis(inner: &str, marker: &str) -> String {
    let (leading, core, trailing) = edge_spaces(inner);
    if core.is_empty() {
        return inner.to_owned();
    }
    format!("{leading}{marker}{core}{marker}{trailing}")
}

fn push_inline_element(element: ElementRef, emphasis: Emphasis, out: &mut String) {
    if is_hidden(element) {
        return;
    }
    let tag = element.value().name();
    let (child_emphasis, marker) = match tag {
        "strong" | "b" if !emphasis.bold => (
            Emphasis {
                bold: true,
                ..emphasis
            },
            Some("**"),
        ),
        "em" | "i" if !emphasis.italic => (
            Emphasis {
                italic: true,
                ..emphasis
            },
            Some("*"),
        ),
        _ => (emphasis, None),
    };
    if tag == "br" {
        out.push(LINE_BREAK);
        return;
    }
    let mut inner = String::new();
    push_inline_children(element, child_emphasis, &mut inner);
    match marker {
        // Emphasis can't span paragraphs, so each part between line breaks is marked on its own.
        Some(marker) => out.push_str(
            &inner
                .split(LINE_BREAK)
                .map(|part| wrap_emphasis(part, marker))
                .collect::<Vec<_>>()
                .join(&LINE_BREAK.to_string()),
        ),
        // A block nested where only inline text is expected must not run into its neighbours.
        None if block_kind(tag).is_some() => {
            out.push(' ');
            out.push_str(&inner);
            out.push(' ');
        }
        None => out.push_str(&inner),
    }
}

fn push_inline_children(element: ElementRef, emphasis: Emphasis, out: &mut String) {
    for node in element.children() {
        match node.value() {
            Node::Text(text) => out.push_str(&escape_inline(text)),
            Node::Element(_) => {
                if let Some(child) = ElementRef::wrap(node) {
                    push_inline_element(child, emphasis, out);
                }
            }
            _ => {}
        }
    }
}

fn inline_text(element: ElementRef) -> String {
    let mut text = String::new();
    push_inline_children(element, Emphasis::default(), &mut text);
    text
}

fn render_list(list: ElementRef, ordered: bool, depth: usize, lines: &mut Vec<String>) {
    let indent = "    ".repeat(depth);
    let items = list
        .child_elements()
        .filter(|item| item.value().name() == "li" && !is_hidden(*item));
    for (index, item) in items.enumerate() {
        let marker = if ordered {
            format!("{}.", index + 1)
        } else {
            "-".to_owned()
        };
        let mut text = String::new();
        let mut nested_lists = Vec::new();
        for node in item.children() {
            match node.value() {
                Node::Text(content) => text.push_str(&escape_inline(content)),
                Node::Element(_) => {
                    let Some(child) = ElementRef::wrap(node) else {
                        continue;
                    };
                    match block_kind(child.value().name()) {
                        Some(Block::List { ordered }) => nested_lists.push((child, ordered)),
                        _ => push_inline_element(child, Emphasis::default(), &mut text),
                    }
                }
                _ => {}
            }
        }
        if let Some(line) = finish_line(&text) {
            lines.push(format!("{indent}{marker} {line}"));
        }
        for (nested, nested_ordered) in nested_lists {
            render_list(nested, nested_ordered, depth + 1, lines);
        }
    }
}

/// A GitHub-style table; its first row is the header, as Markdown tables require one.
fn render_table(table: ElementRef) -> Option<String> {
    let rows: Vec<Vec<String>> = table
        .descendants()
        .filter_map(ElementRef::wrap)
        .filter(|row| row.value().name() == "tr")
        .map(|row| {
            row.child_elements()
                .filter(|cell| matches!(cell.value().name(), "td" | "th"))
                .map(|cell| {
                    collapse_whitespace(&flatten_breaks(&inline_text(cell))).replace('|', r"\|")
                })
                .collect::<Vec<String>>()
        })
        .filter(|cells| cells.iter().any(|cell| !cell.is_empty()))
        .collect();
    let columns = rows.iter().map(Vec::len).max()?;
    let render_row = |cells: &[String]| {
        let padded = (0..columns).map(|column| cells.get(column).map_or("", String::as_str));
        format!("| {} |", padded.collect::<Vec<&str>>().join(" | "))
    };
    let separator = format!("|{}", " --- |".repeat(columns));
    let mut lines = Vec::with_capacity(rows.len() + 1);
    for (index, cells) in rows.iter().enumerate() {
        lines.push(render_row(cells));
        if index == 0 {
            lines.push(separator.clone());
        }
    }
    Some(lines.join("\n"))
}

fn render_preformatted(element: ElementRef) -> Option<String> {
    let code = element.text().collect::<String>();
    let code = code.trim_matches('\n');
    if code.trim().is_empty() {
        return None;
    }
    let longest_backtick_run = code
        .split(|character| character != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest_backtick_run.max(2) + 1);
    Some(format!("{fence}\n{code}\n{fence}"))
}

fn quote_lines(blocks: &[String]) -> String {
    blocks
        .join("\n\n")
        .lines()
        .map(|line| {
            if line.is_empty() {
                ">".to_owned()
            } else {
                format!("> {line}")
            }
        })
        .collect::<Vec<String>>()
        .join("\n")
}

fn render_block(element: ElementRef, block: Block, blocks: &mut Vec<String>) {
    match block {
        Block::Container => collect_blocks(element, blocks),
        Block::Paragraph => blocks.extend(finish_paragraphs(&inline_text(element))),
        Block::Heading(level) => {
            let heading = collapse_whitespace(&flatten_breaks(&inline_text(element)));
            if !heading.is_empty() {
                blocks.push(format!("{} {heading}", "#".repeat(level)));
            }
        }
        Block::List { ordered } => {
            let mut lines = Vec::new();
            render_list(element, ordered, 0, &mut lines);
            if !lines.is_empty() {
                blocks.push(lines.join("\n"));
            }
        }
        Block::Table => blocks.extend(render_table(element)),
        Block::Quote => {
            let mut quoted = Vec::new();
            collect_blocks(element, &mut quoted);
            if !quoted.is_empty() {
                blocks.push(quote_lines(&quoted));
            }
        }
        Block::Preformatted => blocks.extend(render_preformatted(element)),
        Block::Rule => {}
    }
}

/// Loose text and inline elements between blocks form a paragraph of their own.
fn collect_blocks(container: ElementRef, blocks: &mut Vec<String>) {
    let mut run = String::new();
    for node in container.children() {
        match node.value() {
            Node::Text(text) => run.push_str(&escape_inline(text)),
            Node::Element(_) => {
                let Some(child) = ElementRef::wrap(node) else {
                    continue;
                };
                if is_hidden(child) {
                    continue;
                }
                match block_kind(child.value().name()) {
                    Some(block) => {
                        blocks.extend(finish_paragraphs(&std::mem::take(&mut run)));
                        render_block(child, block, blocks);
                    }
                    None => push_inline_element(child, Emphasis::default(), &mut run),
                }
            }
            _ => {}
        }
    }
    blocks.extend(finish_paragraphs(&run));
}

/// Markdown blocks of `container`'s content, in document order; empty blocks are dropped.
pub(super) fn render_blocks(container: ElementRef) -> Vec<String> {
    let mut blocks = Vec::new();
    collect_blocks(container, &mut blocks);
    blocks
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use pretty_assertions::assert_eq;
    use scraper::Html;

    use super::*;
    use crate::parse::selector;

    fn render(article_html: &str) -> Result<Vec<String>> {
        let document = Html::parse_document(&format!("<article>{article_html}</article>"));
        let article = document.select(&selector("article")?).next();
        Ok(article.map(render_blocks).unwrap_or_default())
    }

    #[test]
    fn escape_inline_markup_characters_are_escaped() {
        // Act / Assert
        assert_eq!(escape_inline(r"a*b_c`d[e]f\g<h"), r"a\*b\_c\`d\[e\]f\\g\<h");
        assert_eq!(escape_inline("Artículo 1°- plain."), "Artículo 1°- plain.");
    }

    #[test]
    fn escape_block_start_block_markers_are_escaped() {
        // Act / Assert
        assert_eq!(escape_block_start("# t"), r"\# t");
        assert_eq!(escape_block_start("> q"), r"\> q");
        assert_eq!(escape_block_start("+ i"), r"\+ i");
        assert_eq!(escape_block_start("12. i"), r"12\. i");
        assert_eq!(escape_block_start("3) i"), r"3\) i");
        assert_eq!(escape_block_start("12.5 euros"), "12.5 euros");
        assert_eq!(escape_block_start("Art. 1"), "Art. 1");
    }

    #[test]
    fn render_blocks_nested_emphasis_is_marked_once() -> Result<()> {
        // Act
        let blocks = render("<p><strong>a <b>b</b> c</strong> y <i><em>d</em></i></p>")?;

        // Assert
        assert_eq!(blocks, vec!["**a b c** y *d*".to_owned()]);
        Ok(())
    }

    #[test]
    fn render_blocks_bold_edge_spaces_move_outside_markers() -> Result<()> {
        // Act
        let blocks = render("<p>Art.<strong> 1 </strong>- texto</p>")?;

        // Assert
        assert_eq!(blocks, vec!["Art. **1** - texto".to_owned()]);
        Ok(())
    }

    #[test]
    fn render_blocks_wrapper_div_renders_each_child_block() -> Result<()> {
        // Act
        let blocks = render("<div><p>Uno</p><div><p>Dos</p></div>suelto</div>")?;

        // Assert
        assert_eq!(blocks, vec!["Uno", "Dos", "suelto"]);
        Ok(())
    }

    #[test]
    fn render_blocks_page_text_with_the_break_sentinel_is_not_split() -> Result<()> {
        // Act
        let blocks = render("<p>a\u{E000}\u{E000}b</p><div>c\u{E000} \u{E000}d</div>")?;

        // Assert
        assert_eq!(blocks, vec!["ab", "c d"]);
        Ok(())
    }

    #[test]
    fn render_blocks_single_line_break_reads_as_a_space() -> Result<()> {
        // Act
        let blocks = render("<p>Uno<br>dos</p>suelto<br>texto")?;

        // Assert
        assert_eq!(blocks, vec!["Uno dos", "suelto texto"]);
        Ok(())
    }

    #[test]
    fn render_blocks_double_line_break_separates_paragraphs() -> Result<()> {
        // Act
        let blocks = render("<div>Uno<br><br>Dos<br> <br>Tres<br></div><p>A<br><br>B</p>")?;

        // Assert
        assert_eq!(blocks, vec!["Uno", "Dos", "Tres", "A", "B"]);
        Ok(())
    }

    #[test]
    fn render_blocks_double_line_break_inside_bold_marks_each_paragraph() -> Result<()> {
        // Act
        let blocks = render("<div><strong>Decreto 1<br><br> DECTO-1<br></strong><br> Texto</div>")?;

        // Assert
        assert_eq!(blocks, vec!["**Decreto 1**", "**DECTO-1**", "Texto"]);
        Ok(())
    }

    #[test]
    fn render_blocks_line_break_in_heading_and_cell_reads_as_a_space() -> Result<()> {
        // Act
        let blocks = render("<h2>Capítulo<br><br>I</h2><table><tr><td>A<br>B</td></tr></table>")?;

        // Assert
        assert_eq!(blocks[0], "## Capítulo I");
        assert!(blocks[1].contains("A B"));
        Ok(())
    }

    #[test]
    fn render_blocks_hidden_and_blank_elements_are_dropped() -> Result<()> {
        // Act
        let blocks = render(
            r#"<h2 class="sr-only">Norma</h2><script>x()</script><p> </p><br><hr><p>Texto</p>"#,
        )?;

        // Assert
        assert_eq!(blocks, vec!["Texto"]);
        Ok(())
    }

    #[test]
    fn render_blocks_table_rows_are_padded_to_the_widest_row() -> Result<()> {
        // Act
        let blocks = render("<table><tr><td>a</td></tr><tr><td>b</td><td>c</td></tr></table>")?;

        // Assert
        assert_eq!(blocks, vec!["| a |  |\n| --- | --- |\n| b | c |"]);
        Ok(())
    }

    #[test]
    fn render_blocks_preformatted_text_is_fenced_past_inner_backticks() -> Result<()> {
        // Act
        let blocks = render("<pre>a ``` b</pre>")?;

        // Assert
        assert_eq!(blocks, vec!["````\na ``` b\n````"]);
        Ok(())
    }

    #[test]
    fn render_blocks_blockquote_prefixes_every_line() -> Result<()> {
        // Act
        let blocks = render("<blockquote><p>Uno</p><p>Dos</p></blockquote>")?;

        // Assert
        assert_eq!(blocks, vec!["> Uno\n>\n> Dos"]);
        Ok(())
    }
}
