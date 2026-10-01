//! GitHub Finds: links to GitHub projects, kept in one JSON file and served as one page.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const GITHUB: &str = "https://github.com/";
const INDEX: &str = include_str!("web/index.html");
const APP: &str = include_str!("web/app.js");
const STYLE: &str = include_str!("web/style.css");

/// One find.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Link {
    pub url: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub why: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// YYYY-MM-DD.
    pub added: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stars: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// YYYY-MM-DD of the last push.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pushed: Option<String>,
}

fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

/// Why a link is refused, or None. Only github.com pages, plain text fields of bounded length.
pub fn refuse(link: &Link) -> Option<&'static str> {
    let Some(path) = link.url.strip_prefix(GITHUB) else {
        return Some("url is not a github.com page");
    };
    if path.is_empty()
        || link.url.len() > 300
        || !path
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-._/#?=&%~+".contains(c))
    {
        return Some("url has an empty path, is too long or has a character outside a plain path");
    }
    if link.title.trim().is_empty() || link.title.chars().count() > 120 {
        return Some("title must be 1 to 120 characters");
    }
    if link.why.trim().is_empty() || link.why.chars().count() > 500 {
        return Some("why must be 1 to 500 characters");
    }
    if link
        .summary
        .as_ref()
        .is_some_and(|s| s.chars().count() > 500)
    {
        return Some("summary is over 500 characters");
    }
    if link.tags.len() > 8
        || link.tags.iter().any(|t| {
            t.is_empty()
                || t.len() > 30
                || !t
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
    {
        return Some("tags must be at most 8 of lowercase letters, digits and -");
    }
    if !is_date(&link.added) || link.pushed.as_deref().is_some_and(|d| !is_date(d)) {
        return Some("added and pushed must be YYYY-MM-DD");
    }
    if link
        .language
        .as_ref()
        .is_some_and(|l| l.chars().count() > 40)
    {
        return Some("language is over 40 characters");
    }
    None
}

/// The file's valid links, newest first, and a note for each entry left out.
pub fn parse(text: &str) -> (Vec<Link>, Vec<String>) {
    let rows: Vec<Value> = match serde_json::from_str(text) {
        Ok(Value::Array(rows)) => rows,
        Ok(_) => return (vec![], vec!["the file is not a JSON list".into()]),
        Err(e) => return (vec![], vec![format!("the file is not JSON: {e}")]),
    };
    let mut links = Vec::new();
    let mut notes = Vec::new();
    for (i, row) in rows.into_iter().enumerate() {
        match serde_json::from_value::<Link>(row) {
            Ok(link) => match refuse(&link) {
                None => links.push(link),
                Some(why) => notes.push(format!("entry {i}: {why}")),
            },
            Err(e) => notes.push(format!("entry {i}: {e}")),
        }
    }
    links.sort_by(|a, b| b.added.cmp(&a.added).then_with(|| a.title.cmp(&b.title)));
    (links, notes)
}

/// Add or replace a link by its url; the list stays newest first.
pub fn upsert(links: &mut Vec<Link>, link: Link) -> bool {
    let replaced = if let Some(old) = links.iter_mut().find(|l| l.url == link.url) {
        *old = link;
        true
    } else {
        links.push(link);
        false
    };
    links.sort_by(|a, b| b.added.cmp(&a.added).then_with(|| a.title.cmp(&b.title)));
    replaced
}

/// What the server answers: status, content type, body.
pub struct Reply {
    pub status: u16,
    pub content_type: &'static str,
    pub body: String,
}

fn reply(status: u16, content_type: &'static str, body: impl Into<String>) -> Reply {
    Reply {
        status,
        content_type,
        body: body.into(),
    }
}

/// Answer a GET for `path`, given the links file's text (None when it does not exist yet).
pub fn respond(path: &str, file: Option<&str>) -> Reply {
    let path = path.split('?').next().unwrap_or("");
    match path {
        "/" | "/index.html" => reply(200, "text/html; charset=utf-8", INDEX),
        "/app.js" => reply(200, "text/javascript; charset=utf-8", APP),
        "/style.css" => reply(200, "text/css; charset=utf-8", STYLE),
        "/healthz" => reply(200, "text/plain; charset=utf-8", "ok"),
        "/links.json" => {
            let (links, notes) = file.map(parse).unwrap_or_default();
            let body = serde_json::json!({ "links": links, "left_out": notes });
            reply(200, "application/json", body.to_string())
        }
        _ => reply(404, "text/plain; charset=utf-8", "not found"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(url: &str, added: &str) -> Link {
        Link {
            url: url.into(),
            title: "owner/repo".into(),
            summary: Some("A thing".into()),
            why: "Matches the Proxmox build".into(),
            tags: vec!["proxmox".into()],
            added: added.into(),
            stars: Some(12),
            language: Some("Rust".into()),
            pushed: Some("2026-09-30".into()),
        }
    }

    #[test]
    fn a_github_page_with_plain_fields_is_kept() {
        assert_eq!(
            refuse(&link("https://github.com/owner/repo", "2026-10-01")),
            None
        );
        assert_eq!(
            refuse(&link(
                "https://github.com/owner/repo/tree/main/docs#setup",
                "2026-10-01"
            )),
            None
        );
    }

    #[test]
    fn anything_but_a_plain_github_page_is_refused() {
        for url in [
            "http://github.com/owner/repo",
            "https://github.com/",
            "https://github.com.evil.example/x",
            "https://gitlab.com/owner/repo",
            "javascript:alert(1)",
            "https://github.com/owner/repo\"onclick=x",
            "https://github.com/owner/<script>",
            "https://github.com/owner/repo with space",
        ] {
            assert!(refuse(&link(url, "2026-10-01")).is_some(), "{url}");
        }
    }

    #[test]
    fn bad_fields_are_refused() {
        let mut l = link("https://github.com/o/r", "2026-10-01");
        l.why = " ".into();
        assert!(refuse(&l).is_some());
        let mut l = link("https://github.com/o/r", "01-10-2026");
        assert!(refuse(&l).is_some());
        l.added = "2026-10-01".into();
        l.tags = vec!["Proxmox".into()];
        assert!(refuse(&l).is_some());
        l.tags = (0..9).map(|i| format!("t{i}")).collect();
        assert!(refuse(&l).is_some());
        l.tags = vec![];
        l.pushed = Some("yesterday".into());
        assert!(refuse(&l).is_some());
    }

    #[test]
    fn one_bad_entry_leaves_the_rest_newest_first() {
        let text = serde_json::to_string(&serde_json::json!([
            link("https://github.com/a/old", "2026-09-01"),
            {"url": "https://example.com/x", "title": "t", "why": "w", "added": "2026-10-01"},
            {"title": "no url"},
            link("https://github.com/a/new", "2026-10-01"),
        ]))
        .unwrap();
        let (links, notes) = parse(&text);
        assert_eq!(
            links.iter().map(|l| l.url.as_str()).collect::<Vec<_>>(),
            ["https://github.com/a/new", "https://github.com/a/old"]
        );
        assert_eq!(notes.len(), 2, "{notes:?}");
    }

    #[test]
    fn a_file_that_is_not_a_list_gives_no_links_and_a_note() {
        assert_eq!(parse("{}").0.len(), 0);
        assert_eq!(parse("{}").1.len(), 1);
        assert_eq!(parse("not json").1.len(), 1);
    }

    #[test]
    fn upsert_replaces_by_url() {
        let mut links = vec![link("https://github.com/a/b", "2026-09-01")];
        assert!(!upsert(
            &mut links,
            link("https://github.com/a/c", "2026-10-01")
        ));
        let mut again = link("https://github.com/a/b", "2026-09-01");
        again.why = "Updated".into();
        assert!(upsert(&mut links, again));
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].url, "https://github.com/a/c");
        assert_eq!(links[1].why, "Updated");
    }

    #[test]
    fn routes() {
        assert_eq!(respond("/", None).status, 200);
        assert!(respond("/", None).body.contains("<html"));
        assert_eq!(respond("/healthz", None).body, "ok");
        assert_eq!(respond("/nope", None).status, 404);
        assert_eq!(respond("/../etc/passwd", None).status, 404);
        let empty = respond("/links.json?x=1", None);
        assert_eq!(empty.body, r#"{"left_out":[],"links":[]}"#);
        let text =
            serde_json::to_string(&vec![link("https://github.com/a/b", "2026-10-01")]).unwrap();
        let body: Value = serde_json::from_str(&respond("/links.json", Some(&text)).body).unwrap();
        assert_eq!(body["links"][0]["url"], "https://github.com/a/b");
    }
}
