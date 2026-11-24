//! PRD 009 (plan 235): pij's documentation, compiled into the binary.
//!
//! Each page is a markdown file under `docs/cli/` in the pij repository; its id
//! is its path there without `.md` (so `types/knowledge` is
//! `docs/cli/types/knowledge.md`). The bytes are fixed at build time, so the
//! pages a seat reads match the pij it runs. Reading one starts nothing and
//! contacts no daemon: everything here is pure.

use std::fmt::Write as _;

use pij_core::model::{Envelope, ErrorKind};
use serde_json::{Value, json};

/// One bundled page.
pub struct Page {
    /// The path under `docs/cli/` without `.md`.
    pub id: &'static str,
    /// The page's file, from the repository root.
    pub source: &'static str,
    /// The page's markdown, as built.
    pub body: &'static str,
}

/// A page whose file is `docs/cli/<id>.md`.
macro_rules! cli_page {
    ($id:literal) => {
        Page {
            id: $id,
            source: concat!("docs/cli/", $id, ".md"),
            body: include_str!(concat!("../../../docs/cli/", $id, ".md")),
        }
    };
}

/// The start page `pij agents-start-here` prints.
pub const START_PAGE: &str = "agents-start-here";

/// Every bundled page. A test holds this equal to the files in `docs/cli/`.
pub const PAGES: &[Page] = &[cli_page!("agents-start-here")];

/// `pij docs --help`, which needs nothing.
pub const HELP: &str = "\
pij docs, pij agents-start-here: pij's documentation, bundled in this binary (PRD 009).

Usage:
  pij agents-start-here [--json]          the start page for an agent new to pij
  pij docs list [--json]                  every page, one line each
  pij docs get <page> [--json]            one page
  pij docs get <page>#<heading> [--json]  one section of a page, by its heading's text or slug

No daemon, no network: the pages are the ones this pij was built with.
";

/// A page's title: its first `# ` line.
pub fn title(body: &str) -> &str {
    body.lines()
        .find_map(|line| line.strip_prefix("# "))
        .unwrap_or_default()
        .trim()
}

/// A page's one-line summary: the first paragraph after its title.
pub fn summary(body: &str) -> String {
    body.lines()
        .skip_while(|line| !line.starts_with("# "))
        .skip(1)
        .skip_while(|line| line.trim().is_empty())
        .take_while(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(" ")
}

fn page(id: &str) -> Option<&'static Page> {
    PAGES.iter().find(|page| page.id == id)
}

/// A heading line's level and text, outside fenced code.
struct Heading<'a> {
    line: usize,
    level: usize,
    text: &'a str,
}

fn headings(body: &str) -> Vec<Heading<'_>> {
    let mut fenced = false;
    let mut found = Vec::new();
    for (line, text) in body.lines().enumerate() {
        if text.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        let level = text.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&level)
            && let Some(rest) = text[level..].strip_prefix(' ')
        {
            found.push(Heading {
                line,
                level,
                text: rest.trim(),
            });
        }
    }
    found
}

/// A heading's anchor slug: lowercase, spaces to `-`, other punctuation
/// dropped (GitHub's rule, near enough for a CLI).
pub fn slug(text: &str) -> String {
    text.chars()
        .filter_map(|c| match c {
            ' ' | '-' => Some('-'),
            c if c.is_alphanumeric() || c == '_' => Some(c.to_ascii_lowercase()),
            _ => None,
        })
        .collect()
}

/// The section under `wanted` (its text or slug, any case): from the heading to
/// the next heading of the same or a higher level. `Err` holds every heading's
/// slug, for a suggestion.
fn section(body: &str, wanted: &str) -> Result<String, Vec<String>> {
    let all = headings(body);
    let target = slug(wanted);
    let Some(index) = all
        .iter()
        .position(|h| h.text.eq_ignore_ascii_case(wanted) || slug(h.text) == target)
    else {
        return Err(all.iter().map(|h| slug(h.text)).collect());
    };
    let start = all[index].line;
    let end = all[index + 1..]
        .iter()
        .find(|h| h.level <= all[index].level)
        .map_or(usize::MAX, |h| h.line);
    let lines: Vec<&str> = body
        .lines()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect();
    Ok(lines.join("\n").trim_end().to_string() + "\n")
}

fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let next = (previous + usize::from(ca != *cb))
                .min(row[j] + 1)
                .min(row[j + 1] + 1);
            previous = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}

/// The candidate closest to `wanted`, preferring one that contains it.
pub fn nearest<'a>(wanted: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let wanted = wanted.to_ascii_lowercase();
    candidates.into_iter().min_by_key(|candidate| {
        (
            !candidate.contains(wanted.as_str()),
            distance(&wanted, candidate),
        )
    })
}

/// What one docs call prints and its exit code.
pub struct Outcome {
    /// Everything the call prints, on stdout.
    pub stdout: String,
    /// The process exit code: 0, or 2 for a refusal.
    pub code: u8,
}

fn refused(command: &str, reason: String, json: bool) -> Outcome {
    let envelope = Envelope::<Value>::refused(command, ErrorKind::Refused, reason);
    Outcome {
        stdout: crate::render(&envelope, json) + "\n",
        code: crate::exit_code(&envelope),
    }
}

fn printed(command: &str, text: String, data: Value, json: bool) -> Outcome {
    let stdout = if json {
        crate::render(&Envelope::ok(command, data), true) + "\n"
    } else {
        text
    };
    Outcome { stdout, code: 0 }
}

fn get(query: &str, json: bool) -> Outcome {
    let command = "pij docs get";
    let (id, heading) = match query.split_once('#') {
        Some((id, heading)) => (id, Some(heading)),
        None => (query, None),
    };
    let Some(page) = page(id) else {
        let hint = nearest(id, PAGES.iter().map(|page| page.id))
            .map(|near| format!("; nearest: `pij docs get {near}`"))
            .unwrap_or_default();
        return refused(
            command,
            format!("no page `{id}`{hint}. Run `pij docs list` for every page."),
            json,
        );
    };
    let text = match heading {
        None => page.body.to_string(),
        Some(heading) => match section(page.body, heading) {
            Ok(text) => text,
            Err(slugs) => {
                let hint = nearest(heading, slugs.iter().map(String::as_str))
                    .map(|near| format!("; nearest: `pij docs get {id}#{near}`"))
                    .unwrap_or_default();
                return refused(
                    command,
                    format!("page `{id}` has no heading `{heading}`{hint}."),
                    json,
                );
            }
        },
    };
    let data = json!({ "page": id, "heading": heading, "title": title(page.body), "text": text });
    printed(command, text, data, json)
}

fn list(json: bool) -> Outcome {
    let width = PAGES.iter().map(|page| page.id.len()).max().unwrap_or(0);
    let mut text = String::from("pij docs: the pages bundled with this pij\n\n");
    for page in PAGES {
        let _ = write!(
            text,
            "  {:width$}  {}\n  {:width$}  {}\n",
            page.id,
            title(page.body),
            "",
            summary(page.body)
        );
    }
    text.push_str("\nRun `pij docs get <page>` or `pij docs get <page>#<heading>` to print one.\n");
    let pages: Vec<Value> = PAGES
        .iter()
        .map(|page| {
            json!({
                "page": page.id,
                "title": title(page.body),
                "summary": summary(page.body),
                "source": page.source,
            })
        })
        .collect();
    printed("pij docs list", text, json!({ "pages": pages }), json)
}

/// `pij <verb> <args>` for `verb` `docs` or `agents-start-here`. `json` is the
/// global flag; `--json` among `args` counts too.
pub fn run(verb: &str, args: &[String], json: bool) -> Outcome {
    if pij_core::directory::wants_help(args) || args.first().is_some_and(|arg| arg == "help") {
        return Outcome {
            stdout: HELP.to_string(),
            code: 0,
        };
    }
    let json = json || args.iter().any(|arg| arg == "--json");
    let words: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|arg| *arg != "--json")
        .collect();
    let usage = |command: &str, what: &str| {
        refused(
            command,
            format!("{what}. Run `pij docs --help` for usage."),
            json,
        )
    };
    if verb == START_PAGE {
        return match words.as_slice() {
            [] => get(START_PAGE, json),
            [extra, ..] => usage("pij agents-start-here", &format!("unexpected `{extra}`")),
        };
    }
    match words.as_slice() {
        ["list"] => list(json),
        ["get", query] => get(query, json),
        ["get"] => usage("pij docs get", "name a page: `pij docs get <page>`"),
        [] => usage("pij docs", "name an action: `list` or `get <page>`"),
        [action, ..] => usage(
            "pij docs",
            &format!("unknown action or arguments `{action}`"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_words(verb: &str, words: &[&str]) -> Outcome {
        let args: Vec<String> = words.iter().map(|word| (*word).to_string()).collect();
        run(verb, &args, false)
    }

    const SAMPLE: &str = "# Sample\n\nOne line\nsummary.\n\n## First part\n\nalpha\n\n### Nested bit\n\nbeta\n\n```\n## not a heading\n```\n\n## Second part\n\ngamma\n";

    #[test]
    fn the_table_is_exactly_the_files_in_docs_cli() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/cli");
        fn walk(dir: &std::path::Path, root: &std::path::Path, out: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).expect("read docs/cli") {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    walk(&path, root, out);
                } else if path.extension().is_some_and(|ext| ext == "md") {
                    let id = path.strip_prefix(root).unwrap().with_extension("");
                    out.push(id.to_string_lossy().replace('\\', "/"));
                }
            }
        }
        let mut files = Vec::new();
        walk(&root, &root, &mut files);
        files.sort();
        let mut table: Vec<String> = PAGES.iter().map(|page| page.id.to_string()).collect();
        table.sort();
        assert_eq!(
            table, files,
            "PAGES in docs.rs must list every docs/cli/*.md"
        );
        let repo = root.join("../..");
        for page in PAGES {
            let file = std::fs::read_to_string(repo.join(page.source)).unwrap();
            assert_eq!(page.body, file, "{} differs from {}", page.id, page.source);
            assert!(
                !title(page.body).is_empty(),
                "{} has no `# ` title",
                page.id
            );
            assert!(!summary(page.body).is_empty(), "{} has no summary", page.id);
        }
        assert!(page(START_PAGE).is_some());
    }

    #[test]
    fn title_and_summary_come_from_the_page() {
        assert_eq!(title(SAMPLE), "Sample");
        assert_eq!(summary(SAMPLE), "One line summary.");
    }

    #[test]
    fn a_section_runs_to_the_next_heading_at_its_level_or_above() {
        let first = section(SAMPLE, "First part").unwrap();
        assert!(first.starts_with("## First part\n"));
        assert!(first.contains("### Nested bit") && first.contains("beta"));
        assert!(first.contains("## not a heading"), "fenced lines stay in");
        assert!(!first.contains("gamma"));
        assert_eq!(
            section(SAMPLE, "nested-bit").unwrap(),
            "### Nested bit\n\nbeta\n\n```\n## not a heading\n```\n"
        );
        assert_eq!(
            section(SAMPLE, "SECOND PART").unwrap(),
            "## Second part\n\ngamma\n"
        );
        let missing = section(SAMPLE, "not-a-heading").unwrap_err();
        assert!(
            !missing.contains(&"not-a-heading".to_string()),
            "fenced text is no heading"
        );
    }

    #[test]
    fn nearest_prefers_a_containing_name_then_the_closest() {
        assert_eq!(
            nearest("start", ["concepts", "agents-start-here"]),
            Some("agents-start-here")
        );
        assert_eq!(nearest("concpts", ["concepts", "pods"]), Some("concepts"));
        assert_eq!(nearest("x", Vec::<&str>::new()), None);
    }

    #[test]
    fn the_start_page_prints_with_no_daemon() {
        let out = run_words("agents-start-here", &[]);
        assert_eq!(out.code, 0);
        assert_eq!(out.stdout, page(START_PAGE).unwrap().body);
        let json: Value =
            serde_json::from_str(&run_words("agents-start-here", &["--json"]).stdout).unwrap();
        assert_eq!(json["ok"], true);
        assert_eq!(json["data"]["page"], START_PAGE);
        assert_eq!(run_words("agents-start-here", &["extra"]).code, 2);
    }

    #[test]
    fn list_names_every_page() {
        let out = run_words("docs", &["list"]);
        assert_eq!(out.code, 0);
        for page in PAGES {
            assert!(out.stdout.contains(page.id) && out.stdout.contains(title(page.body)));
        }
        let json: Value =
            serde_json::from_str(&run_words("docs", &["list", "--json"]).stdout).unwrap();
        assert_eq!(json["data"]["pages"].as_array().unwrap().len(), PAGES.len());
        assert_eq!(json["data"]["pages"][0]["source"], PAGES[0].source);
    }

    #[test]
    fn get_prints_a_page_or_one_section() {
        let whole = run_words("docs", &["get", START_PAGE]);
        assert_eq!(
            (whole.code, whole.stdout.as_str()),
            (0, page(START_PAGE).unwrap().body)
        );
        let part = run_words("docs", &["get", "agents-start-here#who-you-are"]);
        assert_eq!(part.code, 0);
        assert!(part.stdout.starts_with("## Who you are\n"));
        assert!(!part.stdout.contains("## Who governs you"));
        let by_text = run_words("docs", &["get", "agents-start-here#Who governs you"]);
        assert!(by_text.stdout.starts_with("## Who governs you\n"));
    }

    #[test]
    fn unknown_pages_and_headings_refuse_with_the_nearest() {
        let page = run_words("docs", &["get", "agents-start"]);
        assert_eq!(page.code, 2);
        assert!(
            page.stdout
                .contains("nearest: `pij docs get agents-start-here`"),
            "{}",
            page.stdout
        );
        assert!(page.stdout.contains("pij docs list"));
        let heading = run_words("docs", &["get", "agents-start-here#who-r-you"]);
        assert_eq!(heading.code, 2);
        assert!(
            heading.stdout.contains("agents-start-here#who-you-are"),
            "{}",
            heading.stdout
        );
    }

    #[test]
    fn usage_errors_refuse_and_help_needs_nothing() {
        for words in [&[][..], &["get"], &["read", "x"], &["get", "a", "b"]] {
            assert_eq!(run_words("docs", words).code, 2, "{words:?}");
        }
        for words in [&["--help"][..], &["-h"], &["help"]] {
            let out = run_words("docs", words);
            assert_eq!((out.code, out.stdout.as_str()), (0, HELP));
        }
        assert_eq!(run_words("agents-start-here", &["--help"]).stdout, HELP);
    }
}
