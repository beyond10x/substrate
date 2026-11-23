//! The site's own stylesheets and the start page's theme script, held to three findings of round
//! R-02. The per-merge gate has no browser, so each rule reads the committed text:
//!
//! - UXF-008: `website/src/css/custom.css` underlines the links of an article's text.
//! - UXF-015: it sets code blocks and table headers of an article to 14 px or more.
//! - UXF-013: the start page reads the theme the docs pages stored before its stylesheet loads,
//!   and `website/static/styles.css` has a light scheme equal to the dark one in its names, used
//!   for a stored `light` and for a system that asks for light, with text at 4.5:1 or more.
use crate::docs::{START_PAGE, read_public};
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const CUSTOM: &str = "website/src/css/custom.css";
const START_STYLES: &str = "website/static/styles.css";

/// The links of an article's text: every link in the Markdown container but a heading's anchor
/// mark and a button. The page list, the header, the footer, the section list and the pagination
/// cards are outside that container.
const ARTICLE_LINKS: &str = ".theme-doc-markdown a:not(.hash-link):not(.button)";
/// The article text the Docs System sets under 14 px, by the selector that outranks its rule.
const ARTICLE_SMALL_TEXT: [&str; 3] = [
    ".theme-doc-markdown .theme-code-block pre[class*='language-']",
    ".theme-doc-markdown.markdown table th",
    ".theme-doc-markdown.markdown table th code",
];
const MIN_TEXT_PX: f64 = 14.0;
const ROOT_PX: f64 = 16.0;

/// The start page's one script, as it stands in the page. The docs pages store the reader's
/// choice in `localStorage` under `theme-07d`: `future.v4` in `website/docusaurus.config.ts`
/// turns on Docusaurus's storage namespace, a three-character hash of the site's `url` and
/// `baseUrl`. A stored `light` or `dark` becomes `data-theme` on the root before the stylesheet
/// loads. With nothing stored, or with storage refused, the attribute stays off and the stylesheet
/// follows `prefers-color-scheme`. `cargo xtask docs-provenance` holds the key to the one the
/// built docs pages read (`docs::start_page_reads_the_docs_theme_key`).
pub(crate) const START_THEME_SCRIPT: &str = "<script>try{var t=localStorage.getItem(\"theme-07d\");if(t===\"light\"||t===\"dark\")document.documentElement.setAttribute(\"data-theme\",t)}catch(e){}</script>";
const STYLESHEET_LINK: &str = "<link rel=\"stylesheet\"";
const DARK: &str = ":root";
const LIGHT: &str = ":root[data-theme=light]";
const SYSTEM_LIGHT: &str = ":root:not([data-theme=dark])";
const SYSTEM_LIGHT_MEDIA: &str = "@media(prefers-color-scheme:light)";
/// The selectors whose links carry no underline: navigation and buttons, never text.
const PLAIN_LINKS: [&str; 4] = ["header a", "footer a", ".button", ".docs-layout nav a"];
/// Text on its surface, as the names of two colours of a scheme. WCAG AA asks 4.5:1.
const TEXT_ON_SURFACE: [(&str, &str); 17] = [
    ("--text", "--bg"),
    ("--muted", "--bg"),
    ("--accent", "--bg"),
    ("--strong", "--bg"),
    ("--text", "--panel"),
    ("--muted", "--panel"),
    ("--text", "--inset"),
    ("--muted", "--inset"),
    ("--accent", "--inset"),
    ("--text", "--well"),
    ("--muted", "--well"),
    ("--accent", "--caption"),
    ("--text", "--code"),
    ("--string", "--code"),
    ("--comment", "--code"),
    ("--bg", "--accent"),
    ("--bg", "--accent-hover"),
];
const MIN_TEXT_CONTRAST: f64 = 4.5;

/// One style rule: the at-rule around it without its spaces (empty at the top level), its
/// selectors and its declarations in source order.
#[derive(Debug, Default, PartialEq, Eq)]
struct Rule {
    media: String,
    selectors: Vec<String>,
    declarations: Vec<(String, String)>,
}
impl Rule {
    fn value(&self, property: &str) -> Option<&str> {
        self.declarations
            .iter()
            .rev()
            .find(|(name, _)| name == property)
            .map(|(_, value)| value.as_str())
    }
    fn custom_properties(&self) -> BTreeMap<&str, &str> {
        self.declarations
            .iter()
            .filter(|(name, _)| name.starts_with("--"))
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect()
    }
}
fn squeeze(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
/// The rules of a stylesheet, those inside an at-rule block included. It reads the two
/// stylesheets this site owns: comments, rules and one level of `@media`.
fn rules(css: &str) -> Vec<Rule> {
    let mut text = String::new();
    let mut rest = css;
    while let Some((before, comment)) = rest.split_once("/*") {
        text.push_str(before);
        rest = comment.split_once("*/").map_or("", |(_, after)| after);
    }
    text.push_str(rest);
    let mut found = Vec::new();
    block(&text, "", &mut found);
    found
}
fn block(mut text: &str, media: &str, found: &mut Vec<Rule>) {
    while let Some((prelude, rest)) = text.split_once('{') {
        let mut depth = 1;
        let end = rest
            .char_indices()
            .find(|(_, c)| {
                match c {
                    '{' => depth += 1,
                    '}' => depth -= 1,
                    _ => (),
                }
                depth == 0
            })
            .map_or(rest.len(), |(at, _)| at);
        let body = &rest[..end];
        let prelude = prelude.trim();
        if prelude.starts_with('@') {
            let name: String = prelude.split_whitespace().collect();
            block(body, &name, found);
        } else {
            found.push(Rule {
                media: media.to_owned(),
                selectors: prelude.split(',').map(squeeze).collect(),
                declarations: body
                    .split(';')
                    .filter_map(|declaration| declaration.split_once(':'))
                    .map(|(name, value)| (name.trim().to_owned(), squeeze(value)))
                    .collect(),
            });
        }
        text = rest.get(end + 1..).unwrap_or_default();
    }
}
/// The top-level rule with exactly this one selector.
fn rule<'a>(rules: &'a [Rule], media: &str, selector: &str) -> Option<&'a Rule> {
    rules
        .iter()
        .find(|rule| rule.media == media && rule.selectors == [selector])
}
/// A length in px, from `px` or from `rem` at the root's 16 px.
fn pixels(value: &str) -> Option<f64> {
    let value = value.trim_end_matches("!important").trim();
    if let Some(rem) = value.strip_suffix("rem") {
        return rem.parse::<f64>().ok().map(|rem| rem * ROOT_PX);
    }
    value.strip_suffix("px")?.parse().ok()
}

/// UXF-008: a link in an article's text is underlined, so it is told from its sentence without
/// colour (WCAG 1.4.1; axe `link-in-text-block`).
fn article_links_are_underlined(css: &str) -> Vec<String> {
    let rules = rules(css);
    let underlined = rule(&rules, "", ARTICLE_LINKS)
        .and_then(|rule| {
            rule.value("text-decoration-line")
                .or_else(|| rule.value("text-decoration"))
        })
        .is_some_and(|value| value.split(' ').any(|word| word == "underline"));
    if underlined {
        Vec::new()
    } else {
        vec![format!(
            "{CUSTOM}: no rule `{ARTICLE_LINKS} {{ text-decoration: underline; }}`: links in article text are told by colour alone (UXF-008)"
        )]
    }
}

/// UXF-015: code blocks and table headers of an article are set at 14 px or more.
fn article_text_is_at_least_14_pixels(css: &str) -> Vec<String> {
    let rules = rules(css);
    let mut failures = Vec::new();
    for selector in ARTICLE_SMALL_TEXT {
        let size = rule(&rules, "", selector).and_then(|rule| rule.value("font-size"));
        match size.and_then(pixels) {
            Some(px) if px >= MIN_TEXT_PX => (),
            Some(px) => failures.push(format!(
                "{CUSTOM}: `{selector}` has font-size {px} px, under {MIN_TEXT_PX} px (UXF-015)"
            )),
            None => failures.push(format!(
                "{CUSTOM}: no rule `{selector}` with a font-size in rem or px of {MIN_TEXT_PX} px or more (UXF-015)"
            )),
        }
    }
    failures
}

/// The relative luminance of `#rrggbb` (WCAG 2.2, relative luminance).
fn luminance(colour: &str) -> Option<f64> {
    let hex = colour.strip_prefix('#').filter(|hex| hex.len() == 6)?;
    let channel = |at: usize| {
        let value = f64::from(u8::from_str_radix(hex.get(at..at + 2)?, 16).ok()?) / 255.0;
        Some(if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        })
    };
    Some(0.2126 * channel(0)? + 0.7152 * channel(2)? + 0.0722 * channel(4)?)
}
fn contrast(a: &str, b: &str) -> Option<f64> {
    let (a, b) = (luminance(a)?, luminance(b)?);
    Some((a.max(b) + 0.05) / (a.min(b) + 0.05))
}
/// Whether a declaration's value states a colour itself instead of naming a scheme's.
fn literal_colour(value: &str) -> bool {
    value.contains('#')
        || value.contains("rgb(")
        || value.contains("hsl(")
        || value
            .split(|c: char| !c.is_ascii_alphabetic())
            .any(|word| word == "white" || word == "black")
}

/// UXF-013: the start page shows the scheme the reader chose on the docs pages, or the system's
/// when none is stored, and its light scheme is complete and readable.
fn start_page_follows_the_chosen_theme(html: &str, css: &str) -> Vec<String> {
    let mut failures = Vec::new();
    let script = html.find(START_THEME_SCRIPT);
    if html.matches("<script").count() != 1 || script.is_none() {
        failures.push(format!(
            "{START_PAGE}: its one script must be the theme script `{START_THEME_SCRIPT}`, which reads the stored theme (UXF-013)"
        ));
    }
    if let (Some(script), Some(link)) = (script, html.find(STYLESHEET_LINK))
        && script > link
    {
        failures.push(format!(
            "{START_PAGE}: the theme script stands after the stylesheet, so the first paint can show the other scheme (UXF-013)"
        ));
    }
    let rules = rules(css);
    let schemes = (
        rule(&rules, "", DARK),
        rule(&rules, "", LIGHT),
        rule(&rules, SYSTEM_LIGHT_MEDIA, SYSTEM_LIGHT),
    );
    let (Some(dark), Some(light), Some(system)) = schemes else {
        failures.push(format!(
            "{START_STYLES}: no light scheme: it needs `{DARK}`, `{LIGHT}` and `{SYSTEM_LIGHT_MEDIA}{{{SYSTEM_LIGHT}}}` (UXF-013)"
        ));
        return failures;
    };
    for (scheme, selector, name) in [(dark, DARK, "dark"), (light, LIGHT, "light")] {
        if scheme.value("color-scheme") != Some(name) {
            failures.push(format!(
                "{START_STYLES}: `{selector}` does not declare `color-scheme:{name}`"
            ));
        }
        failures.extend(unreadable_text(scheme, name));
    }
    if system.declarations != light.declarations {
        failures.push(format!(
            "{START_STYLES}: `{SYSTEM_LIGHT}` under `{SYSTEM_LIGHT_MEDIA}` differs from `{LIGHT}`: a stored light theme and a light system would show different colours"
        ));
    }
    let names = |scheme: &Rule| -> BTreeSet<String> {
        scheme
            .custom_properties()
            .keys()
            .map(|name| (*name).to_owned())
            .collect()
    };
    if names(dark) != names(light) {
        failures.push(format!(
            "{START_STYLES}: the light scheme sets {:?}, the dark scheme {:?}",
            names(light),
            names(dark)
        ));
    }
    let outside = rules.iter().filter(|other| {
        ![dark, light, system]
            .iter()
            .any(|s| std::ptr::eq(*s, *other))
    });
    failures.extend(outside.flat_map(colours_outside_the_schemes));
    failures
}
/// Every text colour of a scheme that is under 4.5:1 on a surface it stands on.
fn unreadable_text(scheme: &Rule, name: &str) -> Vec<String> {
    let colours = scheme.custom_properties();
    let mut failures = Vec::new();
    for (text, surface) in TEXT_ON_SURFACE {
        let ratio = colours
            .get(text)
            .zip(colours.get(surface))
            .and_then(|(text, surface)| contrast(text, surface));
        match ratio {
            Some(ratio) if ratio >= MIN_TEXT_CONTRAST => (),
            Some(ratio) => failures.push(format!(
                "{START_STYLES}: {name} `{text}` on `{surface}` has contrast {ratio:.2}:1, under {MIN_TEXT_CONTRAST}:1"
            )),
            None => failures.push(format!(
                "{START_STYLES}: the {name} scheme does not set `{text}` and `{surface}` as #rrggbb"
            )),
        }
    }
    failures
}
/// What a rule outside the schemes may not do: state a colour of its own, which one scheme would
/// then lack, or take the underline from a link in text.
fn colours_outside_the_schemes(other: &Rule) -> Vec<String> {
    let mut failures = Vec::new();
    let selectors = other.selectors.join(",");
    for (property, value) in &other.declarations {
        if literal_colour(value) {
            failures.push(format!(
                "{START_STYLES}: `{selectors}` sets `{property}:{value}`, a colour outside the two schemes; name a variable both schemes set"
            ));
        }
        let plain = other
            .selectors
            .iter()
            .all(|selector| PLAIN_LINKS.contains(&selector.as_str()));
        if property == "text-decoration" && value == "none" && !plain {
            failures.push(format!(
                "{START_STYLES}: `{selectors}` removes the underline of a link outside {PLAIN_LINKS:?}"
            ));
        }
    }
    failures
}

/// Every failure of the committed stylesheets and the start page, one line each.
pub(crate) fn check(root: &Path) -> Result<Vec<String>> {
    let custom = read_public(root, CUSTOM)?;
    let mut failures = article_links_are_underlined(&custom);
    failures.extend(article_text_is_at_least_14_pixels(&custom));
    failures.extend(start_page_follows_the_chosen_theme(
        &read_public(root, START_PAGE)?,
        &read_public(root, START_STYLES)?,
    ));
    Ok(failures)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn committed(file: &str) -> String {
        read_public(&crate::repo::root().unwrap(), file).unwrap()
    }
    const SCHEME: &str = "--bg:#101010;--panel:#101010;--inset:#101010;--well:#101010;--caption:#101010;--code:#101010;--text:#f0f0f0;--muted:#f0f0f0;--accent:#f0f0f0;--strong:#f0f0f0;--string:#f0f0f0;--comment:#f0f0f0;--accent-hover:#f0f0f0";
    const LIGHT_SCHEME: &str = "--bg:#f0f0f0;--panel:#f0f0f0;--inset:#f0f0f0;--well:#f0f0f0;--caption:#f0f0f0;--code:#f0f0f0;--text:#101010;--muted:#101010;--accent:#101010;--strong:#101010;--string:#101010;--comment:#101010;--accent-hover:#101010";
    fn page() -> String {
        format!(
            "<head><title>t</title>{START_THEME_SCRIPT}<link rel=\"stylesheet\" href=\"/substrate/styles.css\"></head><body></body>"
        )
    }
    fn sheet() -> String {
        format!(
            ":root{{color-scheme:dark;{SCHEME};font-family:x}}:root[data-theme=light]{{color-scheme:light;{LIGHT_SCHEME}}}@media(prefers-color-scheme:light){{:root:not([data-theme=dark]){{color-scheme:light;{LIGHT_SCHEME}}}}}body{{background:var(--bg);color:var(--text)}}a{{color:var(--accent)}}header a,footer a{{text-decoration:none}}@media(max-width:800px){{body{{margin:0}}}}"
        )
    }

    /// UXF-008: the committed stylesheet underlines the links of an article's text, and a
    /// stylesheet without that rule fails.
    #[test]
    fn links_in_article_text_are_underlined() {
        let failures = article_links_are_underlined(&committed(CUSTOM));
        assert!(failures.is_empty(), "{failures:#?}");
        let rule = format!("{ARTICLE_LINKS} {{\n  text-decoration: underline;\n}}\n");
        assert!(article_links_are_underlined(&rule).is_empty());
        for missing in [
            String::new(),
            ".menu__link {\n  min-height: 24px;\n}\n".to_owned(),
            rule.replace("underline", "none"),
            rule.replace(ARTICLE_LINKS, ".theme-doc-markdown a:hover"),
            format!("@media (min-width: 997px) {{\n{rule}}}\n"),
            format!("/* {rule} */"),
        ] {
            assert_eq!(article_links_are_underlined(&missing).len(), 1, "{missing}");
        }
    }
    /// UXF-015: the committed stylesheet sets code blocks and table headers of an article to
    /// 14 px or more, and a smaller or a missing size fails.
    #[test]
    fn code_blocks_and_table_headers_are_at_least_14_pixels() {
        let failures = article_text_is_at_least_14_pixels(&committed(CUSTOM));
        assert!(failures.is_empty(), "{failures:#?}");
        let sheet = |size: &str| {
            let rules: Vec<String> = ARTICLE_SMALL_TEXT
                .iter()
                .map(|selector| format!("{selector} {{\n  font-size: {size};\n}}\n"))
                .collect();
            rules.concat()
        };
        assert!(article_text_is_at_least_14_pixels(&sheet("0.875rem")).is_empty());
        assert!(article_text_is_at_least_14_pixels(&sheet("14px")).is_empty());
        assert_eq!(
            article_text_is_at_least_14_pixels(&sheet("0.82rem")).len(),
            3
        );
        assert_eq!(article_text_is_at_least_14_pixels(&sheet("88%")).len(), 3);
        assert_eq!(article_text_is_at_least_14_pixels("").len(), 3);
        let one = sheet("0.875rem").replacen("0.875rem", "0.68rem", 1);
        assert_eq!(article_text_is_at_least_14_pixels(&one).len(), 1);
    }
    /// UXF-013: the committed start page reads the stored theme before its stylesheet, and its
    /// stylesheet has a complete light scheme for a stored and for a system light. Each way of
    /// losing that fails.
    #[test]
    fn the_start_page_has_a_light_scheme_and_reads_the_stored_theme() {
        let failures =
            start_page_follows_the_chosen_theme(&committed(START_PAGE), &committed(START_STYLES));
        assert!(failures.is_empty(), "{failures:#?}");
        let failing = |html: &str, css: &str| start_page_follows_the_chosen_theme(html, css).len();
        assert_eq!(failing(&page(), &sheet()), 0);
        // The page before the fix: no script, one scheme.
        assert_eq!(
            failing(&page().replace(START_THEME_SCRIPT, ""), &sheet()),
            1
        );
        assert_eq!(
            failing(&page(), &format!(":root{{color-scheme:dark;{SCHEME}}}")),
            1
        );
        let after_styles = page()
            .replace(START_THEME_SCRIPT, "")
            .replace("</head>", &format!("{START_THEME_SCRIPT}</head>"));
        assert_eq!(failing(&after_styles, &sheet()), 1);
        let second = page().replace("<body>", "<body><script src=\"x.js\"></script>");
        assert_eq!(failing(&second, &sheet()), 1);
        let other = page().replace("localStorage.getItem(\"theme-07d\")", "\"dark\"");
        assert_eq!(failing(&other, &sheet()), 1);
        // The stored light theme declared dark: the declaration, and the two light rules differ.
        let dark_light = sheet().replace(
            ":root[data-theme=light]{color-scheme:light",
            ":root[data-theme=light]{color-scheme:dark",
        );
        assert_eq!(failing(&page(), &dark_light), 2);
        // Light muted text at 3.4:1 in both light rules: one line per surface it stands on.
        let pale = sheet().replace("--muted:#101010", "--muted:#808080");
        let pale = start_page_follows_the_chosen_theme(&page(), &pale);
        assert_eq!(pale.len(), 4, "{pale:#?}");
        assert!(
            pale[0].contains("light `--muted` on `--bg` has contrast 3."),
            "{pale:#?}"
        );
        for (broken, why) in [
            (
                sheet().replace("@media(prefers-color-scheme:light)", "@media(print)"),
                "no rule for a light system",
            ),
            (
                sheet().replace("a{color:var(--accent)}", "a{color:#91caff}"),
                "a colour outside the schemes",
            ),
            (
                sheet().replace("a{color:var(--accent)}", "a{color:white}"),
                "a named colour outside the schemes",
            ),
            (
                sheet().replace(
                    "a{color:var(--accent)}",
                    "a{color:var(--accent);text-decoration:none}",
                ),
                "text links lose their underline",
            ),
            (
                sheet().replace("--strong:#f0f0f0;", ""),
                "a colour only the light scheme sets",
            ),
        ] {
            assert!(failing(&page(), &broken) >= 1, "{why}");
        }
    }
    #[test]
    fn a_stylesheet_is_read_minified_and_spaced_with_comments_and_media() {
        let read = rules(
            "/* c { } */\n.a,\n.b  c {\n  color: red;\n  font-size: 1rem;\n}\n@media (max-width: 9px) {\n  .d { margin: 0 }\n}\n.e{x:y;x:z}",
        );
        assert_eq!(read.len(), 3);
        assert_eq!(read[0].selectors, [".a", ".b c"]);
        assert_eq!(read[0].value("font-size"), Some("1rem"));
        assert_eq!(read[1].media, "@media(max-width:9px)");
        assert_eq!(read[1].selectors, [".d"]);
        assert_eq!(read[2].value("x"), Some("z"));
        assert_eq!(pixels("0.875rem"), Some(14.0));
        assert_eq!(pixels("14px"), Some(14.0));
        assert_eq!(pixels("88%"), None);
        let black_on_white = contrast("#000000", "#ffffff").unwrap();
        assert!((black_on_white - 21.0).abs() < 0.01, "{black_on_white}");
        assert!(contrast("#767676", "#ffffff").unwrap() >= 4.5);
        assert!(contrast("#777777", "#ffffff").unwrap() < 4.5);
        assert_eq!(contrast("red", "#ffffff"), None);
    }
}
