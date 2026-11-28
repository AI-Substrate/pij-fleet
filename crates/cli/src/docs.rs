//! PRD 009 (plans 235, 239, 240): pij's documentation, compiled into the binary.
//!
//! A page is either a markdown file in the pij repository (most under
//! `docs/cli/`; `concepts` is the skill's own page) or generated from the
//! daemon's type catalog (`types`, `types/<type>`). Every source is fixed at
//! build time, so the pages a seat reads match the pij it runs. Reading one
//! starts nothing and contacts no daemon: everything here is pure.

use std::fmt::Write as _;
use std::sync::OnceLock;

use pij_core::model::{Envelope, ErrorKind};
use pij_daemon::community::types::{self, TypeDef};
use serde_json::{Value, json};

/// What a page is built from, as compiled in.
pub enum Source {
    /// A file whose text is the page, byte for byte.
    Text {
        /// From the repository root.
        path: String,
        /// The file as built.
        text: &'static str,
    },
    /// A JSON document a page is generated from, as parsed.
    Document {
        /// From the repository root.
        path: String,
        /// The document as built.
        document: &'static Value,
    },
}

impl Source {
    /// The source's file, from the repository root.
    pub fn path(&self) -> &str {
        match self {
            Source::Text { path, .. } | Source::Document { path, .. } => path,
        }
    }

    fn json(&self) -> Value {
        match self {
            Source::Text { path, text } => json!({ "path": path, "text": text }),
            Source::Document { path, document } => json!({ "path": path, "document": document }),
        }
    }
}

/// One bundled page.
pub struct Page {
    /// Its id: `agents-start-here`, `types/knowledge`.
    pub id: String,
    /// The page's markdown.
    pub body: String,
    /// What it is built from; the first is its primary file.
    pub sources: Vec<Source>,
}

fn file_page(id: &str, path: &str, text: &'static str) -> Page {
    Page {
        id: id.to_string(),
        body: text.to_string(),
        sources: vec![Source::Text {
            path: path.to_string(),
            text,
        }],
    }
}

/// A page that is one repository file, given from the repository root.
macro_rules! file_page {
    ($id:literal, $path:literal) => {
        file_page($id, $path, include_str!(concat!("../../../", $path)))
    };
}

fn type_path(def: &TypeDef) -> String {
    format!("templates/types/{}/v{}/type.dd.json", def.id, def.version)
}

fn seed_path(def: &TypeDef) -> String {
    format!("templates/types/{}/v{}/seed.md", def.id, def.version)
}

fn text_of(value: &Value) -> &str {
    value.as_str().unwrap_or("-")
}

/// `types`: the ten, one line each, from the catalog.
fn types_page() -> Page {
    let mut body = String::from(
        "# Seat types\n\npij's ten seat types: how a seat of each lives, one line each. Every role names exactly one.\n\n## The ten\n\n| type | what it is | for a position of authority |\n|---|---|---|\n",
    );
    for row in types::listing()
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let _ = writeln!(
            body,
            "| `{}` | {} | {} |",
            text_of(&row["id"]),
            text_of(&row["line"]),
            if row["authority"] == true {
                "yes"
            } else {
                "no"
            },
        );
    }
    body.push_str(
        "\n## Type, role and position\n\n\
- A role names its type (`pij directory add --type <type>`); a seat's TYPE in `pij list` is the type of the roles it holds.\n\
- A type is a label, never authority. Authority comes from position: the prime designation and the seat roles `pm` and `pa`. The three types marked above describe the seats that hold those positions and grant nothing themselves.\n\
- Types are pij's fixed set: a repo builds any role it needs on one, and changes a type's default rules per role with a reason.\n\
\n## Read one\n\n\
`pij docs get types/<type>` prints a type's purpose, lifecycle, what it keeps, what a role of it must give, its rules and its shipped seed. `pij directory types <type> --seed` shows the seed pij would use in this repo. To build a role on a type: `pij docs get building-a-role`.\n",
    );
    Page {
        id: types::DOCS_PAGE.to_string(),
        body,
        sources: types::all()
            .iter()
            .map(|def| Source::Document {
                path: type_path(def),
                document: &def.document,
            })
            .collect(),
    }
}

/// `types/<type>`: one type's whole definition and its shipped seed.
fn type_page(def: &'static TypeDef) -> Page {
    let show = def.show();
    let mut body = format!(
        "# Type: {}\n\n{}\n\n{}\n\n## Purpose\n\n{}\n\n## Lifecycle\n\n{}\n\n## Keeps\n\n{}\n\n## A role of this type must give\n\n",
        def.id,
        def.line(),
        if def.authority() {
            "This type describes a seat that holds a position of authority. The type itself grants nothing: authority comes from the position (`pij docs get concepts#a-type-is-a-label-never-authority`)."
        } else {
            "Carries no authority."
        },
        text_of(&show["purpose"]),
        text_of(&show["lifecycle"]),
        text_of(&show["keeps"]),
    );
    if def.requires.is_empty() {
        body.push_str("Nothing beyond any role's own fields.\n");
    }
    for field in &def.requires {
        let _ = writeln!(body, "- `{field}`: `--{}`", field.replace('_', "-"));
    }
    body.push_str("\n## Rules\n\n");
    for rule in &def.rules {
        let _ = writeln!(
            body,
            "- `{}` ({}): {}",
            rule.id,
            if rule.fixed { "fixed" } else { "default" },
            rule.text
        );
    }
    let _ = write!(
        body,
        "\nA fixed rule holds for every role of this type. A role may change a default rule with a reason: `pij directory edit <role> --why <text> --type-change <rule>=<why>`.\n\n\
## Seed\n\n\
The opening text for a seat of this type, as pij ships it (version {}). A repo may keep its own: `pij directory types {} --seed` shows the one pij would use there. A seat started in a role of this type (`pij-rs spawn --as role:<id>`) gets the seed in its first turn, with a pointer to this page instead of the definition above.\n\n\
```text\n{}\n```\n\n\
## Source\n\n`{}` and `{}`.\n",
        def.version,
        def.id,
        def.seed.trim_end(),
        type_path(def),
        seed_path(def),
    );
    if def.id == "prime" {
        body.push_str(
            "\nA prime's path from an empty repo to pods: `pij docs get prime-journey`.\n",
        );
    }
    Page {
        id: types::docs_page(&def.id),
        body,
        sources: vec![
            Source::Document {
                path: type_path(def),
                document: &def.document,
            },
            Source::Text {
                path: seed_path(def),
                text: def.seed,
            },
        ],
    }
}

/// The checkout this binary was built from. In a target shared by several
/// worktrees, the bundled-docs check refuses a binary another tree built
/// instead of blaming its pages (review of #348, M1).
pub fn built_from() -> &'static str {
    let crate_dir = env!("CARGO_MANIFEST_DIR");
    crate_dir.strip_suffix("/crates/cli").unwrap_or(crate_dir)
}

/// The start page `pij agents-start-here` prints.
pub const START_PAGE: &str = "agents-start-here";

/// Every bundled page, in the order `pij docs list` shows them.
pub fn pages() -> &'static [Page] {
    static PAGES: OnceLock<Vec<Page>> = OnceLock::new();
    PAGES.get_or_init(|| {
        let mut pages = vec![
            file_page!("agents-start-here", "docs/cli/agents-start-here.md"),
            file_page!("concepts", "skills/pij/references/community/concepts.md"),
            types_page(),
        ];
        pages.extend(types::all().iter().map(type_page));
        pages.push(file_page!("building-a-role", "docs/cli/building-a-role.md"));
        pages.push(file_page!("files", "docs/cli/files.md"));
        pages.push(file_page!("pods", "docs/cli/pods.md"));
        pages.push(file_page!("prime-journey", "docs/cli/prime-journey.md"));
        pages
    })
}

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
    pages().iter().find(|page| page.id == id)
}

/// A heading line's level and text, outside fenced code.
struct Heading<'a> {
    line: usize,
    level: usize,
    text: &'a str,
}

/// ATX headings outside ``` and ~~~ fences, a closing `#` run dropped.
/// Setext headings, headings indented 1 to 3 spaces and non-ASCII case
/// folding are not handled (plan 235); no bundled page uses them.
fn headings(body: &str) -> Vec<Heading<'_>> {
    let mut fence: Option<&str> = None;
    let mut found = Vec::new();
    for (line, text) in body.lines().enumerate() {
        let trimmed = text.trim_start();
        if let Some(marker) = ["```", "~~~"].into_iter().find(|m| trimmed.starts_with(m)) {
            match fence {
                None => fence = Some(marker),
                Some(open) if open == marker => fence = None,
                Some(_) => {}
            }
            continue;
        }
        if fence.is_some() {
            continue;
        }
        let level = text.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&level)
            && let Some(rest) = text[level..].strip_prefix(' ')
        {
            let rest = rest.trim();
            let closed = rest.trim_end_matches('#');
            let rest = if closed.len() < rest.len() && (closed.is_empty() || closed.ends_with(' '))
            {
                closed.trim_end()
            } else {
                rest
            };
            found.push(Heading {
                line,
                level,
                text: rest,
            });
        }
    }
    found
}

/// Each heading's slug; a repeated slug gets `-1`, `-2`, as GitHub does.
fn slugs(all: &[Heading<'_>]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    all.iter()
        .map(|heading| {
            let base = slug(heading.text);
            let repeats = seen.iter().filter(|other| **other == base).count();
            seen.push(base.clone());
            if repeats == 0 {
                base
            } else {
                format!("{base}-{repeats}")
            }
        })
        .collect()
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
    let slugs = slugs(&all);
    let target = slug(wanted);
    let Some(index) = slugs
        .iter()
        .position(|s| *s == target)
        .or_else(|| all.iter().position(|h| h.text.eq_ignore_ascii_case(wanted)))
    else {
        return Err(slugs);
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

/// The candidate closest to `wanted`, preferring one that contains it; none
/// when nothing is close (an edit distance above a third of its length, at
/// least 2), so a refusal never suggests a page that has nothing to do with it.
pub fn nearest<'a>(wanted: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let wanted = wanted.to_ascii_lowercase();
    let cap = (wanted.chars().count() / 3).max(2);
    candidates
        .into_iter()
        .map(|candidate| {
            let contains = wanted.chars().count() >= 3 && candidate.contains(wanted.as_str());
            (!contains, distance(&wanted, candidate), candidate)
        })
        .filter(|(far, distance, _)| !far || *distance <= cap)
        .min_by_key(|(far, distance, _)| (*far, *distance))
        .map(|(.., candidate)| candidate)
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
        let hint = nearest(id, pages().iter().map(|page| page.id.as_str()))
            .map(|near| format!("; nearest: `pij docs get {near}`"))
            .unwrap_or_default();
        return refused(
            command,
            format!("no page `{id}`{hint}. Run `pij docs list` for every page."),
            json,
        );
    };
    let text = match heading {
        None => page.body.clone(),
        Some(heading) => match section(&page.body, heading) {
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
    let sources: Vec<Value> = page.sources.iter().map(Source::json).collect();
    let data = json!({
        "page": id,
        "heading": heading,
        "title": title(&page.body),
        "text": text,
        "sources": sources,
    });
    printed(command, text, data, json)
}

fn list(json: bool) -> Outcome {
    let width = pages().iter().map(|page| page.id.len()).max().unwrap_or(0);
    let mut text = String::from("pij docs: the pages bundled with this pij\n\n");
    for page in pages() {
        let _ = write!(
            text,
            "  {:width$}  {}\n  {:width$}  {}\n",
            page.id,
            title(&page.body),
            "",
            summary(&page.body)
        );
    }
    text.push_str("\nRun `pij docs get <page>` or `pij docs get <page>#<heading>` to print one.\n");
    let listed: Vec<Value> = pages()
        .iter()
        .map(|page| {
            json!({
                "page": page.id,
                "title": title(&page.body),
                "summary": summary(&page.body),
                "source": page.sources.first().map(Source::path),
                "sources": page.sources.iter().map(Source::path).collect::<Vec<_>>(),
            })
        })
        .collect();
    printed(
        "pij docs list",
        text,
        json!({ "pages": listed, "built_from": built_from() }),
        json,
    )
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
    fn every_source_is_its_file_and_every_docs_cli_page_is_bundled() {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        fn walk(dir: &std::path::Path, repo: &std::path::Path, out: &mut Vec<String>) {
            for entry in std::fs::read_dir(dir).expect("read docs/cli") {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    walk(&path, repo, out);
                } else if path.extension().is_some_and(|ext| ext == "md") {
                    let relative = path.strip_prefix(repo).unwrap();
                    out.push(relative.to_string_lossy().replace('\\', "/"));
                }
            }
        }
        let mut files = Vec::new();
        walk(&repo.join("docs/cli"), &repo, &mut files);
        let sources: Vec<&str> = pages()
            .iter()
            .flat_map(|page| page.sources.iter().map(Source::path))
            .collect();
        for file in &files {
            assert!(sources.contains(&file.as_str()), "{file} is not bundled");
        }
        for page in pages() {
            assert!(!page.sources.is_empty(), "{} has no source", page.id);
            for source in &page.sources {
                let file = std::fs::read_to_string(repo.join(source.path()))
                    .unwrap_or_else(|_| panic!("{}: {} is missing", page.id, source.path()));
                match source {
                    Source::Text { text, .. } => assert_eq!(*text, file, "{}", source.path()),
                    Source::Document { document, .. } => assert_eq!(
                        **document,
                        serde_json::from_str::<Value>(&file).unwrap(),
                        "{}",
                        source.path()
                    ),
                }
            }
            assert!(
                !title(&page.body).is_empty(),
                "{} has no `# ` title",
                page.id
            );
            assert!(
                !summary(&page.body).is_empty(),
                "{} has no summary",
                page.id
            );
        }
        let ids: Vec<&str> = pages().iter().map(|page| page.id.as_str()).collect();
        let mut unique = ids.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), ids.len(), "page ids are unique");
        assert!(page(START_PAGE).is_some());
    }

    /// PRD 009 req-05: the first pages, and concepts defines every term.
    #[test]
    fn the_first_pages_are_bundled_and_concepts_defines_the_seven_terms() {
        for id in [
            "agents-start-here",
            "concepts",
            "types",
            "building-a-role",
            "pods",
            "prime-journey",
        ] {
            assert!(page(id).is_some(), "{id}");
        }
        for kind in types::TYPES {
            assert!(page(&format!("types/{kind}")).is_some(), "types/{kind}");
        }
        let concepts = &page("concepts").unwrap().body;
        for term in [
            "seat", "role", "type", "position", "pod", "seed", "template",
        ] {
            assert!(
                concepts.contains(&format!("| {term} |")),
                "concepts defines {term}"
            );
        }
    }

    #[test]
    fn a_type_page_is_its_definition_and_seed() {
        let def = types::get("knowledge").unwrap();
        let body = &page("types/knowledge").unwrap().body;
        assert_eq!(title(body), "Type: knowledge");
        assert_eq!(summary(body), def.line());
        for rule in &def.rules {
            assert!(body.contains(&format!("- `{}` (fixed): {}", rule.id, rule.text)));
        }
        assert!(body.contains("- `keeps`: `--keeps`"));
        assert!(body.contains(def.seed.trim_end()));
        assert!(body.contains("Carries no authority."));
        let prime = &page("types/prime").unwrap().body;
        assert!(prime.contains("The type itself grants nothing"));
        let rules = section(body, "rules").unwrap();
        assert!(rules.starts_with("## Rules\n") && !rules.contains("## Seed"));
        let overview = &page("types").unwrap().body;
        for def in types::all() {
            assert!(overview.contains(&format!("| `{}` | {} |", def.id, def.line())));
        }
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
        // Review of #348, M5: nothing close, no suggestion.
        assert_eq!(
            nearest("nope", ["types", "concepts", "agents-start-here"]),
            None
        );
        assert_eq!(
            nearest("e", ["types"]),
            None,
            "a short name contains nothing"
        );
    }

    /// Review of #348, M3: ~~~ fences, a closing `#` run, repeated headings.
    #[test]
    fn tilde_fences_closing_hashes_and_repeated_headings() {
        let body = "# T\n\nx\n\n## Part ##\n\none\n\n~~~\n## fenced\n```\n## still fenced\n~~~\n\n## Part\n\ntwo\n\n## C#\n";
        assert_eq!(section(body, "part").unwrap().lines().nth(2), Some("one"));
        assert!(section(body, "part").unwrap().contains("## still fenced"));
        assert_eq!(section(body, "part-1").unwrap(), "## Part\n\ntwo\n");
        assert_eq!(section(body, "C#").unwrap(), "## C#\n");
        let missing = section(body, "fenced").unwrap_err();
        assert_eq!(missing, ["t", "part", "part-1", "c"]);
    }

    #[test]
    fn the_start_page_prints_with_no_daemon() {
        let out = run_words("agents-start-here", &[]);
        assert_eq!(out.code, 0);
        assert_eq!(out.stdout, page(START_PAGE).unwrap().body.as_str());
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
        for page in pages() {
            assert!(out.stdout.contains(&page.id) && out.stdout.contains(title(&page.body)));
        }
        let json: Value =
            serde_json::from_str(&run_words("docs", &["list", "--json"]).stdout).unwrap();
        assert_eq!(
            json["data"]["pages"].as_array().unwrap().len(),
            pages().len()
        );
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        assert_eq!(
            std::fs::canonicalize(json["data"]["built_from"].as_str().unwrap()).unwrap(),
            std::fs::canonicalize(repo).unwrap()
        );
        assert_eq!(
            json["data"]["pages"][0]["source"],
            "docs/cli/agents-start-here.md"
        );
        let get: Value =
            serde_json::from_str(&run_words("docs", &["get", "types/coder", "--json"]).stdout)
                .unwrap();
        assert_eq!(
            get["data"]["sources"][0]["path"],
            "templates/types/coder/v1/type.dd.json"
        );
        assert!(get["data"]["sources"][0]["document"].is_object());
        assert!(get["data"]["sources"][1]["text"].is_string());
    }

    #[test]
    fn get_prints_a_page_or_one_section() {
        let whole = run_words("docs", &["get", START_PAGE]);
        assert_eq!(
            (whole.code, whole.stdout.as_str()),
            (0, page(START_PAGE).unwrap().body.as_str())
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
        let far = run_words("docs", &["get", "nope"]);
        assert_eq!(far.code, 2);
        assert!(!far.stdout.contains("nearest"), "{}", far.stdout);
        assert!(far.stdout.contains("pij docs list"));
        let page = run_words("docs", &["get", "agents-start"]);
        assert_eq!(page.code, 2);
        assert!(
            page.stdout
                .contains("nearest: `pij docs get agents-start-here`"),
            "{}",
            page.stdout
        );
        assert!(page.stdout.contains("pij docs list"));
        let heading = run_words("docs", &["get", "agents-start-here#who-you-r"]);
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
