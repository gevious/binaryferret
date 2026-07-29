//! A small, well-scoped Markdown → Typst converter (FR-19, local publish path).
//!
//! It covers the subset a BinaryFerret vault actually uses: ATX headings, bold /
//! italic, inline code, fenced code blocks, bullet + ordered lists, links,
//! images, `[[wiki-links]]`, block quotes and horizontal rules. Anything else
//! is passed through as escaped literal text, so an unsupported construct
//! degrades to plain text rather than producing broken Typst.
//!
//! The conversion is a pure string→string transform with no I/O, which is where
//! the fiddly correctness lives — hence it is unit-tested construct by construct.

/// Typst markup characters that must be escaped when they appear in literal text.
const SPECIAL: &[char] = &['\\', '#', '$', '*', '_', '`', '<', '>', '@', '[', ']', '~'];

fn escape_char(c: char) -> String {
    if SPECIAL.contains(&c) {
        format!("\\{c}")
    } else {
        c.to_string()
    }
}

fn escape_text(s: &str) -> String {
    s.chars().map(escape_char).collect()
}

/// Escape for inclusion in a Typst string literal ("...").
fn escape_str(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn char_at(chars: &[char], i: usize) -> Option<char> {
    chars.get(i).copied()
}

fn starts_with(chars: &[char], i: usize, pat: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    i + p.len() <= chars.len() && chars[i..i + p.len()] == p[..]
}

fn find_char(chars: &[char], from: usize, target: char) -> Option<usize> {
    (from..chars.len()).find(|&j| chars[j] == target)
}

fn find_str(chars: &[char], from: usize, pat: &str) -> Option<usize> {
    let p: Vec<char> = pat.chars().collect();
    (from..chars.len()).find(|&j| j + p.len() <= chars.len() && chars[j..j + p.len()] == p[..])
}

/// A char boundary for emphasis detection: start-of-run or a non-alphanumeric
/// neighbour. Prevents `snake_case` from being read as italic.
fn boundary_before(chars: &[char], i: usize) -> bool {
    i == 0 || !chars[i - 1].is_alphanumeric()
}
fn boundary_after(chars: &[char], i: usize) -> bool {
    char_at(chars, i)
        .map(|c| !c.is_alphanumeric())
        .unwrap_or(true)
}

/// Parse a `[text](url)` starting at `chars[start] == '['`. Returns
/// `(text, url, index_after_close_paren)`. Non-nested by design.
fn parse_link(chars: &[char], start: usize) -> Option<(String, String, usize)> {
    let text_close = find_char(chars, start + 1, ']')?;
    if char_at(chars, text_close + 1) != Some('(') {
        return None;
    }
    let url_close = find_char(chars, text_close + 2, ')')?;
    let text: String = chars[start + 1..text_close].iter().collect();
    let url: String = chars[text_close + 2..url_close].iter().collect();
    Some((text, url, url_close + 1))
}

/// Try to match one span construct at `chars[i]`. On success returns the Typst
/// rendering plus the index just past the consumed input; otherwise `None`.
fn span_at(chars: &[char], i: usize) -> Option<(String, usize)> {
    let c = chars[i];

    // inline code — content is literal, no escaping needed inside Typst raw
    if c == '`' {
        if let Some(close) = find_char(chars, i + 1, '`') {
            let content: String = chars[i + 1..close].iter().collect();
            return Some((format!("`{content}`"), close + 1));
        }
    }

    // image ![alt](url)
    if c == '!' && char_at(chars, i + 1) == Some('[') {
        if let Some((_alt, url, next)) = parse_link(chars, i + 1) {
            return Some((format!("#image(\"{}\")", escape_str(&url)), next));
        }
    }

    // wiki-link [[Target]] — rendered underlined (no navigation in a PDF)
    if starts_with(chars, i, "[[") {
        if let Some(close) = find_str(chars, i + 2, "]]") {
            let content: String = chars[i + 2..close].iter().collect();
            return Some((format!("#underline[{}]", escape_text(&content)), close + 2));
        }
    }

    // link [text](url)
    if c == '[' {
        if let Some((text, url, next)) = parse_link(chars, i) {
            return Some((
                format!("#link(\"{}\")[{}]", escape_str(&url), inline(&text)),
                next,
            ));
        }
    }

    // bold **text** / __text__
    for marker in ["**", "__"] {
        if !starts_with(chars, i, marker) {
            continue;
        }
        let underscore = marker == "__";
        if underscore && !boundary_before(chars, i) {
            continue;
        }
        if let Some(close) = find_str(chars, i + 2, marker) {
            if close > i + 2 && (!underscore || boundary_after(chars, close + 2)) {
                let content: String = chars[i + 2..close].iter().collect();
                return Some((format!("*{}*", inline(&content)), close + 2));
            }
        }
    }

    // italic *text* / _text_
    if c == '*' || c == '_' {
        let underscore = c == '_';
        if !underscore || boundary_before(chars, i) {
            if let Some(close) = find_char(chars, i + 1, c) {
                if close > i + 1 && (!underscore || boundary_after(chars, close + 1)) {
                    let content: String = chars[i + 1..close].iter().collect();
                    return Some((format!("_{}_", inline(&content)), close + 1));
                }
            }
        }
    }

    None
}

/// Convert inline (span-level) Markdown to Typst.
fn inline(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        match span_at(&chars, i) {
            Some((rendered, next)) => {
                out.push_str(&rendered);
                i = next;
            }
            None => {
                out.push_str(&escape_char(chars[i]));
                i += 1;
            }
        }
    }
    out
}

fn is_fence(line: &str) -> bool {
    line.starts_with("```") || line.starts_with("~~~")
}

fn is_hr(line: &str) -> bool {
    let l = line.trim();
    (l.len() >= 3 && l.chars().all(|c| c == '-'))
        || (l.len() >= 3 && l.chars().all(|c| c == '*'))
        || (l.len() >= 3 && l.chars().all(|c| c == '_'))
}

fn heading(line: &str) -> Option<String> {
    let hashes = line.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&hashes) && line[hashes..].starts_with(' ') {
        let rest = line[hashes + 1..].trim_start();
        return Some(format!("{} {}", "=".repeat(hashes), inline(rest)));
    }
    None
}

fn blockquote(line: &str) -> Option<String> {
    let rest = line.strip_prefix("> ").or_else(|| line.strip_prefix(">"))?;
    Some(format!("#quote(block: true)[{}]", inline(rest.trim())))
}

fn list_item(line: &str) -> Option<String> {
    let indent_len = line.len() - line.trim_start().len();
    let indent = &line[..indent_len];
    let rest = &line[indent_len..];
    for m in ["- ", "* ", "+ "] {
        if let Some(after) = rest.strip_prefix(m) {
            return Some(format!("{indent}- {}", inline(after.trim_end())));
        }
    }
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() {
        if let Some(after) = rest[digits.len()..].strip_prefix(". ") {
            return Some(format!("{indent}+ {}", inline(after.trim_end())));
        }
    }
    None
}

/// Convert a Markdown document body to Typst markup (no preamble).
pub fn to_typst_body(md: &str) -> String {
    let mut out = String::new();
    let mut in_fence = false;
    for line in md.lines() {
        let trimmed = line.trim_end();
        if is_fence(trimmed) {
            in_fence = !in_fence;
            out.push_str(trimmed);
            out.push('\n');
            continue;
        }
        if in_fence {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        if trimmed.is_empty() {
            out.push('\n');
        } else if let Some(h) = heading(trimmed) {
            out.push_str(&h);
            out.push('\n');
        } else if is_hr(trimmed) {
            out.push_str("#line(length: 100%)\n");
        } else if let Some(q) = blockquote(trimmed) {
            out.push_str(&q);
            out.push('\n');
        } else if let Some(l) = list_item(line) {
            out.push_str(&l);
            out.push('\n');
        } else {
            out.push_str(&inline(trimmed));
            out.push('\n');
        }
    }
    out
}

/// Wrap a converted body in a minimal, font-safe Typst document. No custom font
/// is set so the compile works with Typst's embedded fonts on a bare machine.
pub fn document(md: &str, title: Option<&str>) -> String {
    let mut s = String::new();
    s.push_str("#set page(margin: 2.5cm)\n");
    s.push_str("#set par(justify: true)\n");
    s.push_str("#set text(size: 11pt)\n");
    if let Some(t) = title {
        s.push_str(&format!("#set document(title: \"{}\")\n", escape_str(t)));
    }
    s.push('\n');
    s.push_str(&to_typst_body(md));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(md: &str) -> String {
        to_typst_body(md)
    }

    #[test]
    fn headings_map_to_equals_levels() {
        assert!(body("# Title").contains("= Title"));
        assert!(body("### Deep").contains("=== Deep"));
        // seven hashes is not a heading — stays escaped text
        assert!(!body("####### x").contains("======="));
    }

    #[test]
    fn bold_and_italic() {
        assert_eq!(inline("**bold**"), "*bold*");
        assert_eq!(inline("*em*"), "_em_");
        assert_eq!(inline("__strong__"), "*strong*");
    }

    #[test]
    fn underscores_inside_words_are_literal_not_italic() {
        // must not become emphasis; the underscores are escaped literals
        assert_eq!(inline("foo_bar_baz"), "foo\\_bar\\_baz");
    }

    #[test]
    fn links_images_and_wikilinks() {
        assert_eq!(
            inline("[docs](https://x.dev)"),
            "#link(\"https://x.dev\")[docs]"
        );
        assert_eq!(inline("![a](img.png)"), "#image(\"img.png\")");
        assert_eq!(inline("[[Some Note]]"), "#underline[Some Note]");
    }

    #[test]
    fn inline_code_is_literal() {
        assert_eq!(inline("run `git status` now"), "run `git status` now");
    }

    #[test]
    fn special_chars_in_plain_text_are_escaped() {
        assert_eq!(inline("cost is $5 for #1"), "cost is \\$5 for \\#1");
    }

    #[test]
    fn bullet_and_ordered_lists() {
        assert!(body("- one").contains("- one"));
        assert!(body("* two").contains("- two"));
        assert!(body("1. first").contains("+ first"));
    }

    #[test]
    fn fenced_code_passes_through_verbatim() {
        let md = "```rust\nlet x = *y; // stars & _underscores_\n```";
        let out = body(md);
        assert!(out.contains("```rust"));
        // content inside the fence is untouched (not escaped)
        assert!(out.contains("let x = *y; // stars & _underscores_"));
    }

    #[test]
    fn horizontal_rule_and_blockquote() {
        assert!(body("---").contains("#line(length: 100%)"));
        assert!(body("> quoted").contains("#quote(block: true)[quoted]"));
    }

    #[test]
    fn document_has_preamble_and_title() {
        let doc = document("# Hi", Some("My Doc"));
        assert!(doc.contains("#set page(margin: 2.5cm)"));
        assert!(doc.contains("#set document(title: \"My Doc\")"));
        assert!(doc.contains("= Hi"));
    }
}
