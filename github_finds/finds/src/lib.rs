//! GitHub Finds: links to GitHub projects, kept in one JSON file and served as one page.

use std::net::{IpAddr, Ipv4Addr};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use discover::Dismiss;

pub mod discover;

pub const GITHUB: &str = "https://github.com/";
const INDEX: &str = include_str!("web/index.html");
const APP: &str = include_str!("web/app.js");
const STYLE: &str = include_str!("web/style.css");
/// Home Assistant's ingress proxy, the one source allowed to delete a find.
pub const INGRESS: IpAddr = IpAddr::V4(Ipv4Addr::new(172, 30, 32, 2));
/// The most a delete request's body may hold.
pub const MAX_BODY: usize = 4096;
const TEXT: &str = "text/plain; charset=utf-8";

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

/// Remove the link with this url; whether one was there.
pub fn remove(links: &mut Vec<Link>, url: &str) -> bool {
    let before = links.len();
    links.retain(|l| l.url != url);
    links.len() != before
}

/// Owner and repository from a github.com url, trailing slash ignored.
pub fn owner_repo(url: &str) -> Option<(String, String)> {
    let mut parts = url.strip_prefix(GITHUB)?.trim_end_matches('/').split('/');
    let owner = parts.next().filter(|s| !s.is_empty())?;
    let repo = parts.next().filter(|s| !s.is_empty())?;
    Some((owner.to_string(), repo.to_string()))
}

/// A link from the GitHub API's repository object; tags given win over the first four topics.
pub fn from_repo(
    url: &str,
    why: &str,
    tags: Option<Vec<String>>,
    meta: &Value,
    added: &str,
) -> Link {
    let topics = || {
        meta["topics"]
            .as_array()
            .map(|t| {
                t.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .take(4)
                    .collect()
            })
            .unwrap_or_default()
    };
    Link {
        url: url.trim_end_matches('/').to_string(),
        title: meta["full_name"]
            .as_str()
            .map(String::from)
            .or_else(|| owner_repo(url).map(|(o, r)| format!("{o}/{r}")))
            .unwrap_or_default(),
        summary: meta["description"]
            .as_str()
            .map(|s| s.chars().take(500).collect()),
        why: why.to_string(),
        tags: tags.unwrap_or_else(topics),
        added: added.to_string(),
        stars: meta["stargazers_count"].as_u64(),
        language: meta["language"].as_str().map(String::from),
        pushed: meta["pushed_at"]
            .as_str()
            .map(|s| s.chars().take(10).collect()),
    }
}

/// YYYY-MM-DD for a count of days since 1970-01-01 (Howard Hinnant's civil_from_days).
pub fn date_from_days(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
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

/// Whether a request came from the trusted source; one with no address did not.
pub fn from_trusted(remote: Option<IpAddr>, trusted: IpAddr) -> bool {
    remote == Some(trusted)
}

/// Answer a GET for `path`, given the links file's text (None when it does not exist yet), the
/// sentence saying what discovery does, and whether this requester may delete finds.
pub fn respond(path: &str, file: Option<&str>, discovery: &str, can_remove: bool) -> Reply {
    let path = path.split('?').next().unwrap_or("");
    match path {
        "/" | "/index.html" => reply(200, "text/html; charset=utf-8", INDEX),
        "/app.js" => reply(200, "text/javascript; charset=utf-8", APP),
        "/style.css" => reply(200, "text/css; charset=utf-8", STYLE),
        "/healthz" => reply(200, "text/plain; charset=utf-8", "ok"),
        "/links.json" => {
            let (links, notes) = file.map(parse).unwrap_or_default();
            let body = serde_json::json!({
                "links": links, "left_out": notes, "discovery": discovery, "can_remove": can_remove
            });
            reply(200, "application/json", body.to_string())
        }
        _ => reply(404, TEXT, "not found"),
    }
}

/// Answer `POST /remove`, whose JSON body names a find's url as in the links file: the find is
/// deleted and kept out of discovery. Only the trusted source may; the LAN port stays read-only.
pub fn respond_remove(
    trusted: bool,
    content_type: Option<&str>,
    body: &[u8],
    links: &str,
    state: &str,
) -> Reply {
    if !trusted {
        return reply(403, TEXT, "finds are deleted only through Home Assistant");
    }
    let json = content_type
        .and_then(|c| c.split(';').next())
        .is_some_and(|c| c.trim().eq_ignore_ascii_case("application/json"));
    if !json {
        return reply(400, TEXT, "Content-Type must be application/json");
    }
    if body.len() > MAX_BODY {
        return reply(400, TEXT, "body over 4 KB");
    }
    let url = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| v.get("url")?.as_str().map(String::from));
    let Some(url) = url else {
        return reply(400, TEXT, r#"body must be {"url": "<url>"}"#);
    };
    match discover::dismiss(links, state, &url) {
        Ok(n) => reply(
            200,
            "application/json",
            serde_json::json!({ "count": n }).to_string(),
        ),
        Err(Dismiss::NotListed) => reply(404, TEXT, "no find has that url"),
        Err(Dismiss::Failed(e)) => reply(500, TEXT, e),
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
    fn each_limit_holds_at_its_edge() {
        let ok = link("https://github.com/o/r", "2026-10-01");
        let mut l = ok.clone();
        l.url = format!("https://github.com/{}", "a".repeat(300 - GITHUB.len()));
        assert_eq!(refuse(&l), None);
        l.url.push('a');
        assert!(refuse(&l).is_some());
        let mut l = ok.clone();
        l.title = "t".repeat(120);
        assert_eq!(refuse(&l), None);
        l.title.push('t');
        assert!(refuse(&l).is_some());
        let mut l = ok.clone();
        l.title = "  ".into();
        assert!(refuse(&l).is_some());
        let mut l = ok.clone();
        l.why = "w".repeat(500);
        assert_eq!(refuse(&l), None);
        l.why.push('w');
        assert!(refuse(&l).is_some());
        let mut l = ok.clone();
        l.summary = Some("s".repeat(500));
        assert_eq!(refuse(&l), None);
        l.summary = Some("s".repeat(501));
        assert!(refuse(&l).is_some());
        let mut l = ok.clone();
        l.tags = (0..8).map(|i| format!("t{i}")).collect();
        assert_eq!(refuse(&l), None);
        l.tags = vec!["a".repeat(30)];
        assert_eq!(refuse(&l), None);
        l.tags = vec!["a".repeat(31)];
        assert!(refuse(&l).is_some());
        l.tags = vec![String::new()];
        assert!(refuse(&l).is_some());
        let mut l = ok.clone();
        l.language = Some("l".repeat(40));
        assert_eq!(refuse(&l), None);
        l.language = Some("l".repeat(41));
        assert!(refuse(&l).is_some());
    }

    #[test]
    fn a_date_needs_both_dashes_and_digits_elsewhere() {
        for bad in [
            "2026x10-01",
            "2026-10x01",
            "2026-1a-01",
            "2026-10-1",
            "2026-10-011",
        ] {
            let l = link("https://github.com/o/r", bad);
            assert!(refuse(&l).is_some(), "{bad}");
        }
    }

    #[test]
    fn dates_from_days() {
        assert_eq!(date_from_days(0), "1970-01-01");
        assert_eq!(date_from_days(59), "1970-03-01");
        assert_eq!(date_from_days(10_957), "2000-01-01");
        assert_eq!(date_from_days(11_016), "2000-02-29");
        assert_eq!(date_from_days(11_017), "2000-03-01");
        assert_eq!(date_from_days(20_727), "2026-10-01");
        assert_eq!(date_from_days(20_819), "2027-01-01");
        assert_eq!(date_from_days(-1), "1969-12-31");
    }

    #[test]
    fn owner_and_repo_from_a_url() {
        assert_eq!(
            owner_repo("https://github.com/o/r/"),
            Some(("o".into(), "r".into()))
        );
        assert_eq!(
            owner_repo("https://github.com/o/r/tree/main"),
            Some(("o".into(), "r".into()))
        );
        assert_eq!(owner_repo("https://github.com/o"), None);
        assert_eq!(owner_repo("https://github.com//r"), None);
        assert_eq!(owner_repo("https://gitlab.com/o/r"), None);
    }

    #[test]
    fn a_link_from_the_api() {
        let meta = serde_json::json!({
            "full_name": "Owner/Repo", "description": "d".repeat(600), "stargazers_count": 42,
            "language": "Rust", "pushed_at": "2026-09-30T12:00:00Z", "topics": ["a", "b", "c", "d", "e"]
        });
        let l = from_repo(
            "https://github.com/Owner/Repo/",
            "w",
            None,
            &meta,
            "2026-10-01",
        );
        assert_eq!(l.url, "https://github.com/Owner/Repo");
        assert_eq!(l.title, "Owner/Repo");
        assert_eq!(l.summary.as_ref().map(|s| s.len()), Some(500));
        assert_eq!(l.tags, ["a", "b", "c", "d"]);
        assert_eq!(
            (l.stars, l.language.as_deref(), l.pushed.as_deref()),
            (Some(42), Some("Rust"), Some("2026-09-30"))
        );
        let given = from_repo(
            "https://github.com/o/r",
            "w",
            Some(vec!["x".into()]),
            &serde_json::json!({}),
            "2026-10-01",
        );
        assert_eq!(
            (given.title.as_str(), given.tags.as_slice()),
            ("o/r", ["x".to_string()].as_slice())
        );
        assert_eq!(given.summary, None);
    }

    #[test]
    fn remove_by_url() {
        let mut links = vec![
            link("https://github.com/a/b", "2026-09-01"),
            link("https://github.com/a/c", "2026-10-01"),
        ];
        assert!(!remove(&mut links, "https://github.com/a/x"));
        assert!(remove(&mut links, "https://github.com/a/b"));
        assert_eq!(
            links.iter().map(|l| l.url.as_str()).collect::<Vec<_>>(),
            ["https://github.com/a/c"]
        );
    }

    #[test]
    fn routes() {
        assert_eq!(
            respond("/app.js", None, "", false).content_type,
            "text/javascript; charset=utf-8"
        );
        assert!(
            respond("/app.js", None, "", false)
                .body
                .contains("links.json")
        );
        assert_eq!(
            respond("/style.css", None, "", false).content_type,
            "text/css; charset=utf-8"
        );
        assert!(respond("/style.css", None, "", false).body.contains("--bg"));
        assert_eq!(respond("/", None, "", false).status, 200);
        assert!(respond("/", None, "", false).body.contains("<html"));
        assert_eq!(respond("/healthz", None, "", false).body, "ok");
        assert_eq!(respond("/nope", None, "", false).status, 404);
        assert_eq!(respond("/../etc/passwd", None, "", false).status, 404);
        let empty = respond("/links.json?x=1", None, "Daily discovery is off.", false);
        assert_eq!(
            empty.body,
            r#"{"can_remove":false,"discovery":"Daily discovery is off.","left_out":[],"links":[]}"#
        );
        let text =
            serde_json::to_string(&vec![link("https://github.com/a/b", "2026-10-01")]).unwrap();
        let body: Value =
            serde_json::from_str(&respond("/links.json", Some(&text), "", false).body).unwrap();
        assert_eq!(body["links"][0]["url"], "https://github.com/a/b");
    }

    #[test]
    fn only_the_ingress_proxy_is_told_it_can_remove() {
        let lan: IpAddr = "127.0.0.1".parse().unwrap();
        assert!(from_trusted(Some(INGRESS), INGRESS));
        assert!(!from_trusted(Some(lan), INGRESS));
        assert!(!from_trusted(None, INGRESS));
        let can = |trusted: bool| {
            let body: Value =
                serde_json::from_str(&respond("/links.json", None, "", trusted).body).unwrap();
            body["can_remove"].clone()
        };
        assert_eq!(can(true), Value::Bool(true));
        assert_eq!(can(false), Value::Bool(false));
    }

    /// A links file with two finds and no state file, in a folder of its own.
    fn files(name: &str) -> (String, String) {
        let dir = std::env::temp_dir().join(format!("finds-lib-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let links = dir.join("links.json").to_string_lossy().into_owned();
        let two = vec![
            link("https://github.com/a/Keep", "2026-09-01"),
            link("https://github.com/a/Gone", "2026-10-01"),
        ];
        std::fs::write(&links, serde_json::to_string(&two).unwrap()).unwrap();
        (
            links,
            dir.join("discover.json").to_string_lossy().into_owned(),
        )
    }

    const JSON: Option<&str> = Some("application/json");
    const GONE: &[u8] = br#"{"url": "https://github.com/a/Gone"}"#;

    #[test]
    fn a_delete_from_outside_ingress_is_refused_and_changes_nothing() {
        let (links, state) = files("lan");
        let before = std::fs::read_to_string(&links).unwrap();
        assert_eq!(
            respond_remove(false, JSON, GONE, &links, &state).status,
            403
        );
        assert_eq!(std::fs::read_to_string(&links).unwrap(), before);
        assert!(!std::path::Path::new(&state).exists());
        let _ = std::fs::remove_dir_all(std::path::Path::new(&links).parent().unwrap());
    }

    #[test]
    fn a_delete_through_ingress_removes_the_find_and_records_the_dismissal() {
        let (links, state) = files("ingress");
        let r = respond_remove(
            true,
            Some("application/json; charset=utf-8"),
            GONE,
            &links,
            &state,
        );
        assert_eq!((r.status, r.body.as_str()), (200, r#"{"count":1}"#));
        let (left, _) = parse(&std::fs::read_to_string(&links).unwrap());
        assert_eq!(
            left.iter().map(|l| l.url.as_str()).collect::<Vec<_>>(),
            ["https://github.com/a/Keep"]
        );
        let saved: discover::State =
            serde_json::from_str(&std::fs::read_to_string(&state).unwrap()).unwrap();
        assert!(saved.dismissed.contains("https://github.com/a/gone"));
        let _ = std::fs::remove_dir_all(std::path::Path::new(&links).parent().unwrap());
    }

    #[test]
    fn a_delete_of_an_unknown_url_is_404_and_changes_nothing() {
        let (links, state) = files("unknown");
        let before = std::fs::read_to_string(&links).unwrap();
        let body = br#"{"url": "https://github.com/a/Never"}"#;
        assert_eq!(respond_remove(true, JSON, body, &links, &state).status, 404);
        assert_eq!(std::fs::read_to_string(&links).unwrap(), before);
        assert!(!std::path::Path::new(&state).exists());
        let _ = std::fs::remove_dir_all(std::path::Path::new(&links).parent().unwrap());
    }

    #[test]
    fn a_bad_delete_request_is_400_and_changes_nothing() {
        let (links, state) = files("bad");
        let before = std::fs::read_to_string(&links).unwrap();
        let big = format!(
            r#"{{"url": "https://github.com/a/Gone", "pad": "{}"}}"#,
            "x".repeat(MAX_BODY)
        );
        for (ct, body) in [
            (JSON, b"not json".as_slice()),
            (JSON, br#"{"link": "https://github.com/a/Gone"}"#.as_slice()),
            (JSON, br#"{"url": 3}"#.as_slice()),
            (JSON, br#"["https://github.com/a/Gone"]"#.as_slice()),
            (JSON, big.as_bytes()),
            (Some("text/plain"), GONE),
            (None, GONE),
        ] {
            let r = respond_remove(true, ct, body, &links, &state);
            assert_eq!(r.status, 400, "{ct:?} {}", String::from_utf8_lossy(body));
        }
        assert_eq!(std::fs::read_to_string(&links).unwrap(), before);
        assert!(!std::path::Path::new(&state).exists());
        let _ = std::fs::remove_dir_all(std::path::Path::new(&links).parent().unwrap());
    }
}
