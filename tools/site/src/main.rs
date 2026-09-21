//! Generates the project site from the repository's own documentation.
//!
//! The guides on the site are rendered from `docs/*.md` rather than written twice,
//! because this project has spent real effort keeping documentation verifiable
//! against the code — a hand-copied site would drift from it within a week.
//!
//! ```sh
//! cargo run -p stanchion-site -- --out site-out
//! ```

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use pulldown_cmark::{html, Options, Parser};

/// Where guide links that leave the site point instead.
const REPO: &str = "https://github.com/jalcine/stanchion";

/// The domain the site is served from.
const DOMAIN: &str = "stanchion.olamaelcu.net";

/// Order the guides appear in, following the reading order a newcomer wants rather
/// than the alphabetical order the filesystem gives.
const GUIDE_ORDER: &[&str] = &[
    "lua-class",
    "registry",
    "isolation",
    "capabilities",
    "signatures",
    "dependencies",
    "luarocks",
    "remote",
    "distribution",
    "index-server",
    "bindings",
    "testing",
];

fn main() -> ExitCode {
    match run() {
        Ok(out) => {
            println!("site written to {}", out.display());
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("stanchion-site: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<PathBuf, String> {
    let mut args = std::env::args().skip(1);
    let mut out = PathBuf::from("site-out");
    let mut root = repo_root();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => out = PathBuf::from(args.next().ok_or("--out needs a path")?),
            "--root" => root = PathBuf::from(args.next().ok_or("--root needs a path")?),
            "-h" | "--help" => {
                println!("stanchion-site [--root DIR] [--out DIR]");
                return Ok(out);
            }
            other => return Err(format!("unexpected argument `{other}`")),
        }
    }

    let guides = read_guides(&root)?;
    write_site(&root, &out, &guides)?;
    Ok(out)
}

/// The repository root, derived from where this crate lives.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// One rendered guide.
struct Guide {
    slug: String,
    title: String,
    body: String,
}

/// Reads every `docs/*.md`, in the order a reader wants them.
fn read_guides(root: &Path) -> Result<Vec<Guide>, String> {
    let dir = root.join("docs");
    let mut found: BTreeMap<String, String> = BTreeMap::new();

    let entries = fs::read_dir(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    for entry in entries {
        let path = entry.map_err(|err| err.to_string())?.path();
        if path.extension().is_some_and(|ext| ext == "md")
            && let Some(slug) = path.file_stem().and_then(|stem| stem.to_str())
        {
            let source =
                fs::read_to_string(&path).map_err(|err| format!("{}: {err}", path.display()))?;
            found.insert(slug.to_string(), source);
        }
    }

    // Listed guides first in their reading order, then anything new so a guide added
    // later still appears rather than being silently dropped.
    let mut ordered: Vec<String> = GUIDE_ORDER
        .iter()
        .filter(|slug| found.contains_key(**slug))
        .map(|slug| (*slug).to_string())
        .collect();
    for slug in found.keys() {
        if !ordered.contains(slug) {
            ordered.push(slug.clone());
        }
    }

    let mut guides = Vec::new();
    for slug in ordered {
        let Some(source) = found.get(&slug) else { continue };
        guides.push(Guide {
            title: title_of(source).unwrap_or_else(|| slug.clone()),
            body: render(&rewrite_links(source)),
            slug,
        });
    }
    Ok(guides)
}

/// A document's own `# ` heading, which is the title it should carry on the site.
fn title_of(source: &str) -> Option<String> {
    source
        .lines()
        .find_map(|line| line.strip_prefix("# "))
        .map(|title| title.trim().to_string())
}

/// Points links at where they live on the site, or at the repository.
///
/// Done on the Markdown rather than the rendered HTML: the link targets are
/// unambiguous here, and a regex over generated markup is how a generator starts
/// mangling code samples that merely look like links.
fn rewrite_links(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut rest = source;

    while let Some(start) = rest.find("](") {
        let (before, after) = rest.split_at(start.saturating_add(2));
        out.push_str(before);
        match after.find(')') {
            None => {
                rest = after;
                break;
            }
            Some(end) => {
                let (target, remainder) = after.split_at(end);
                out.push_str(&rewrite_target(target));
                rest = remainder;
            }
        }
    }
    out.push_str(rest);
    out
}

fn rewrite_target(target: &str) -> String {
    // Leave absolute and in-page links alone.
    if target.starts_with("http://") || target.starts_with("https://") || target.starts_with('#') {
        return target.to_string();
    }

    let (path, anchor) = match target.split_once('#') {
        Some((path, anchor)) => (path, format!("#{anchor}")),
        None => (target, String::new()),
    };

    // The documentation index lives on the landing page.
    if path == "../README.md" {
        return format!("../index.html{anchor}");
    }
    // A sibling guide is a sibling page.
    if let Some(slug) = path.strip_suffix(".md")
        && !slug.contains('/')
    {
        return format!("{slug}.html{anchor}");
    }
    // Anything else lives in the repository, not on the site. GitHub serves a
    // directory under `tree` and a file under `blob`, and gets shirty about the
    // wrong one, so the last segment having an extension decides which.
    let cleaned = path.trim_start_matches("../").trim_end_matches('/');
    let is_file = cleaned
        .rsplit('/')
        .next()
        .is_some_and(|segment| segment.contains('.'));
    let kind = if is_file { "blob" } else { "tree" };
    format!("{REPO}/{kind}/main/{cleaned}{anchor}")
}

/// Markdown to HTML, with the extensions the documentation actually uses.
fn render(source: &str) -> String {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_SMART_PUNCTUATION);

    let mut html = String::new();
    html::push_html(&mut html, Parser::new_ext(source, options));
    html
}

/// The shared navigation, with the current page marked.
fn nav(guides: &[Guide], current: Option<&str>) -> String {
    let mut out = String::from("<nav class=\"guides\" aria-label=\"Guides\">");
    for guide in guides {
        let here = current == Some(guide.slug.as_str());
        let mark = if here { " aria-current=\"page\"" } else { "" };
        let prefix = if current.is_some() { "" } else { "docs/" };
        out.push_str(&format!(
            "<a href=\"{prefix}{}.html\"{mark}>{}</a>",
            guide.slug, guide.title
        ));
    }
    out.push_str("</nav>");
    out
}

fn write_site(root: &Path, out: &Path, guides: &[Guide]) -> Result<(), String> {
    let write = |path: PathBuf, contents: &str| -> Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| format!("{}: {err}", parent.display()))?;
        }
        fs::write(&path, contents).map_err(|err| format!("{}: {err}", path.display()))
    };

    // Served as-is: Jekyll would only slow the build and surprise us.
    write(out.join(".nojekyll"), "")?;
    write(out.join("CNAME"), &format!("{DOMAIN}\n"))?;
    write(out.join("assets/site.css"), include_str!("../assets/site.css"))?;
    write(out.join("assets/stanchion.svg"), include_str!("../assets/stanchion.svg"))?;

    let landing = include_str!("../templates/index.html")
        .replace("{{nav}}", &nav(guides, None))
        .replace("{{repo}}", REPO);
    write(out.join("index.html"), &landing)?;

    let shell = include_str!("../templates/page.html");
    for guide in guides {
        let page = shell
            .replace("{{title}}", &guide.title)
            .replace("{{nav}}", &nav(guides, Some(&guide.slug)))
            .replace("{{content}}", &guide.body)
            .replace(
                "{{source}}",
                &format!(
                    "<a href=\"{REPO}/blob/main/docs/{}.md\">Edit this page on GitHub</a>",
                    guide.slug
                ),
            )
            .replace("{{repo}}", REPO);
        write(out.join("docs").join(format!("{}.html", guide.slug)), &page)?;
    }

    // An index of the guides, so `/docs/` is not a dead end.
    let mut list = String::from("<ul class=\"guide-index\">");
    for guide in guides {
        list.push_str(&format!(
            "<li><a href=\"{}.html\"><strong>{}</strong></a></li>",
            guide.slug, guide.title
        ));
    }
    list.push_str("</ul>");
    let index = shell
        .replace("{{title}}", "Guides")
        .replace("{{nav}}", &nav(guides, Some("")))
        .replace("{{content}}", &list)
        // This page has no Markdown behind it, so it points at the directory the
        // others come from rather than a `docs/index.md` that does not exist.
        .replace(
            "{{source}}",
            &format!("<a href=\"{REPO}/tree/main/docs\">These guides on GitHub</a>"),
        )
        .replace("{{repo}}", REPO);
    write(out.join("docs/index.html"), &index)?;

    // Proof the generator read the real thing rather than a copy.
    let readme = root.join("README.md");
    if !readme.is_file() {
        return Err(format!("{} is missing", readme.display()));
    }
    Ok(())
}
