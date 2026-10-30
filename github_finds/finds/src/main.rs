//! `github-finds serve` runs in the add-on and searches for finds once a day; `discover` runs one
//! search now. `add`, `list` and `remove` edit the links file on the Home Assistant host over SSH
//! from a workstation, taking a repository's details from `gh api`.

use std::cell::Cell;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use github_finds::discover::{self, Fetch, Net, Options, Paths};
use github_finds::{
    GITHUB, Link, date_from_days, from_repo, owner_repo, parse, refuse, remove as remove_url,
    respond, upsert,
};
use serde_json::Value;
use tiny_http::{Header, Method, Response, Server};

const USAGE: &str = "usage:
  github-finds serve [--links /config/links.json] [--port 8091] [--options /data/options.json] [--state /data/discover.json]
  github-finds discover [--links ...] [--options ...] [--state ...] [--dry-run]
  github-finds add <github url> --why <text> [--tags a,b] --host <user@host> [--ssh-port 22] [--key <file>] [--dir <folder>]
  github-finds list | remove <github url>   (same --host, --ssh-port, --key, --dir)
The SSH settings may instead come from GITHUB_FINDS_HOST, GITHUB_FINDS_SSH_PORT, GITHUB_FINDS_KEY
and GITHUB_FINDS_DIR; a flag wins over its variable.";
/// The add-on's config folder on the host when it is installed from the GitHub repository.
const DIR: &str = "/addon_configs/75df1782_github_finds";

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("serve") => serve(&args),
        Some("discover") => discover_now(&args),
        Some("add") => add(&args),
        Some("list") => list(&args),
        Some("remove") => remove(&args),
        _ => {
            eprintln!("{USAGE}");
            2
        }
    };
    std::process::exit(code);
}

fn serve(args: &[String]) -> i32 {
    let links = flag(args, "--links").unwrap_or_else(|| "/config/links.json".into());
    let port = flag(args, "--port").unwrap_or_else(|| "8091".into());
    let server = match Server::http(format!("0.0.0.0:{port}")) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot listen on {port}: {e}");
            return 1;
        }
    };
    let (n, notes) = std::fs::read_to_string(&links)
        .map(|t| parse(&t))
        .map(|(l, n)| (l.len(), n))
        .unwrap_or((0, vec![]));
    eprintln!("github-finds listening on {port}, {n} links in {links}");
    for note in notes {
        eprintln!("left out: {note}");
    }
    let discovery = match paths(args, &links) {
        Some(paths) => {
            let said = options(&paths).describe();
            std::thread::spawn(move || discover_loop(&paths));
            said
        }
        None => "Daily discovery is off.".to_string(),
    };
    eprintln!("{discovery}");
    let headers = [
        (
            "Content-Security-Policy",
            "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'self'",
        ),
        ("X-Content-Type-Options", "nosniff"),
        ("Referrer-Policy", "no-referrer"),
        ("Cache-Control", "no-store"),
    ];
    for request in server.incoming_requests() {
        let reply = if matches!(request.method(), Method::Get | Method::Head) {
            let file = std::fs::read_to_string(&links).ok();
            respond(request.url(), file.as_deref(), &discovery)
        } else {
            github_finds::Reply {
                status: 405,
                content_type: "text/plain; charset=utf-8",
                body: "GET only".into(),
            }
        };
        let mut response = Response::from_string(reply.body).with_status_code(reply.status);
        for (k, v) in headers
            .iter()
            .chain([("Content-Type", reply.content_type)].iter())
        {
            if let Ok(h) = Header::from_bytes(k.as_bytes(), v.as_bytes()) {
                response.add_header(h);
            }
        }
        let _ = request.respond(response);
    }
    0
}

/// Discovery's files; None without `--options`.
fn paths(args: &[String], links: &str) -> Option<Paths> {
    Some(Paths {
        links: links.to_string(),
        options: flag(args, "--options")?,
        state: flag(args, "--state").unwrap_or_else(|| "/data/discover.json".into()),
    })
}

fn options(paths: &Paths) -> Options {
    paths.options().unwrap_or_else(|e| {
        eprintln!("discover: options not valid, using defaults: {e}");
        Options::default()
    })
}

fn epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// GitHub's search, unauthenticated (10 a minute, so one every 7 s), and the router's chat endpoint.
struct Http {
    agent: ureq::Agent,
    last_search: Cell<Option<Instant>>,
}

const SEARCH_GAP: Duration = Duration::from_secs(7);
const AGENT: &str = concat!("ha-github-finds/", env!("CARGO_PKG_VERSION"));

impl Http {
    fn new() -> Http {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(90)))
            .http_status_as_error(false)
            .build()
            .into();
        Http {
            agent,
            last_search: Cell::new(None),
        }
    }
}

fn answer(result: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<Value, Fetch> {
    let mut resp = result.map_err(|e| Fetch::Down(e.to_string()))?;
    let status = resp.status().as_u16();
    let text = resp
        .body_mut()
        .read_to_string()
        .map_err(|e| Fetch::Down(e.to_string()))?;
    discover::classify(status, &text)
}

impl Net for Http {
    fn search(&self, query: &str) -> Result<Value, Fetch> {
        if let Some(wait) = self
            .last_search
            .get()
            .and_then(|t| SEARCH_GAP.checked_sub(t.elapsed()))
        {
            std::thread::sleep(wait);
        }
        self.last_search.set(Some(Instant::now()));
        answer(
            self.agent
                .get("https://api.github.com/search/repositories")
                .header("User-Agent", AGENT)
                .header("Accept", "application/vnd.github+json")
                .query("q", query)
                .query("sort", "stars")
                .query("per_page", "10")
                .call(),
        )
    }

    fn models(&self, router: &str) -> Result<Value, Fetch> {
        answer(
            self.agent
                .get(format!("{router}/models"))
                .header("User-Agent", AGENT)
                .call(),
        )
    }

    fn chat(&self, router: &str, body: &Value) -> Result<Value, Fetch> {
        answer(
            self.agent
                .post(format!("{router}/chat/completions"))
                .header("User-Agent", AGENT)
                .header("Content-Type", "application/json")
                .send(body.to_string()),
        )
    }
}

/// One run, logged; whether it finished.
fn discover_once(net: &dyn Net, paths: &Paths, dry: bool) -> bool {
    match discover::once(net, &options(paths), paths, dry, epoch()) {
        Ok(run) => {
            if dry {
                for (title, pick, score, why) in &run.report.verdicts {
                    println!(
                        "{}\t{score}\t{title}\t{why}",
                        if *pick { "pick" } else { "no" }
                    );
                }
                eprintln!("{} (dry run, nothing written)", run.report);
            } else {
                eprintln!("{}", run.report);
            }
            run.finished
        }
        Err(e) => {
            eprintln!("discover: {e}");
            false
        }
    }
}

/// Check every 30 minutes, the first two minutes after start; run when a day has passed.
fn discover_loop(paths: &Paths) {
    let net = Http::new();
    std::thread::sleep(Duration::from_secs(120));
    loop {
        if discover::should_run(&options(paths), &paths.state(), epoch()) {
            discover_once(&net, paths, false);
        }
        std::thread::sleep(Duration::from_secs(1800));
    }
}

fn discover_now(args: &[String]) -> i32 {
    let links = flag(args, "--links").unwrap_or_else(|| "/config/links.json".into());
    let mut with_default: Vec<String> = args.to_vec();
    if flag(args, "--options").is_none() {
        with_default.extend(["--options".into(), "/data/options.json".into()]);
    }
    let Some(paths) = paths(&with_default, &links) else {
        return 2;
    };
    let dry = args.iter().any(|a| a == "--dry-run");
    if discover_once(&Http::new(), &paths, dry) {
        0
    } else {
        1
    }
}

struct Remote {
    host: String,
    port: String,
    key: Option<String>,
    dir: String,
}

/// A folder name safe to put in the remote shell command unquoted.
fn plain_dir(d: &str) -> bool {
    d.starts_with('/')
        && d.len() > 1
        && !d.contains("..")
        && d.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_./-".contains(c))
}

impl Remote {
    /// The SSH target from the flags, else the environment; the host has no default.
    fn from(args: &[String], env: impl Fn(&str) -> Option<String>) -> Result<Remote, String> {
        let pick = |name: &str, var: &str| {
            flag(args, name).or_else(|| env(var).filter(|v| !v.trim().is_empty()))
        };
        let host = pick("--host", "GITHUB_FINDS_HOST")
            .ok_or("no host: pass --host user@host or set GITHUB_FINDS_HOST")?;
        if host.starts_with('-') {
            return Err(format!("host {host} would read as an ssh option"));
        }
        let port = pick("--ssh-port", "GITHUB_FINDS_SSH_PORT").unwrap_or_else(|| "22".into());
        if port.parse::<u16>().is_err() {
            return Err(format!("ssh port {port} is not a port number"));
        }
        let dir = pick("--dir", "GITHUB_FINDS_DIR").unwrap_or_else(|| DIR.into());
        if !plain_dir(&dir) {
            return Err(format!(
                "folder {dir} must be an absolute path of letters, digits and _ . / -"
            ));
        }
        Ok(Remote {
            host,
            port,
            key: pick("--key", "GITHUB_FINDS_KEY"),
            dir: dir.trim_end_matches('/').to_string(),
        })
    }

    fn run(&self, command: &str, input: Option<&[u8]>) -> Result<String, String> {
        let mut ssh = Command::new("ssh");
        if let Some(key) = &self.key {
            ssh.args(["-i", key]);
        }
        let mut child = ssh
            .args(["-p", &self.port, "-o", "BatchMode=yes", &self.host, command])
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("ssh: {e}"))?;
        if let (Some(bytes), Some(mut stdin)) = (input, child.stdin.take()) {
            stdin
                .write_all(bytes)
                .map_err(|e| format!("ssh stdin: {e}"))?;
        }
        let mut out = String::new();
        if let Some(mut stdout) = child.stdout.take() {
            stdout
                .read_to_string(&mut out)
                .map_err(|e| format!("ssh stdout: {e}"))?;
        }
        let status = child.wait().map_err(|e| format!("ssh: {e}"))?;
        if !status.success() {
            return Err(format!("ssh exited {status}"));
        }
        Ok(out)
    }

    /// The links on the host; an absent file is an empty list. Markers keep a login banner out.
    fn read(&self) -> Result<Vec<Link>, String> {
        let dir = &self.dir;
        let out = self.run(
            &format!("{AS_ROOT}; echo __FINDS__; $S cat {dir}/links.json 2>/dev/null || echo '[]'; echo __END__"),
            None,
        )?;
        let body = out
            .split("__FINDS__")
            .nth(1)
            .and_then(|s| s.split("__END__").next())
            .ok_or("no marked output from the host")?;
        let (links, notes) = parse(body.trim());
        if !notes.is_empty() {
            return Err(format!(
                "the file on the host has entries this tool would drop: {notes:?}"
            ));
        }
        Ok(links)
    }

    fn write(&self, links: &[Link]) -> Result<(), String> {
        let json = serde_json::to_string_pretty(links).map_err(|e| e.to_string())? + "\n";
        let dir = &self.dir;
        self.run(
            &format!("{AS_ROOT}; $S sh -c 'mkdir -p {dir} && cat > {dir}/.links.tmp && chmod 644 {dir}/.links.tmp && mv {dir}/.links.tmp {dir}/links.json'"),
            Some(json.as_bytes()),
        )
        .map(|_| ())
    }
}

/// Root reads the add-on's folder directly; any other user goes through `sudo -n`.
const AS_ROOT: &str = r#"S=; [ "$(id -u)" = 0 ] || S="sudo -n""#;

fn remote(args: &[String]) -> Result<Remote, String> {
    Remote::from(args, |v| std::env::var(v).ok())
}

/// Today's UTC date as YYYY-MM-DD.
fn today() -> String {
    date_from_days(i64::try_from(epoch() / 86_400).unwrap_or(0))
}

fn gh_repo(owner: &str, repo: &str) -> Result<serde_json::Value, String> {
    let out = Command::new("gh")
        .args(["api", &format!("repos/{owner}/{repo}")])
        .output()
        .map_err(|e| format!("gh: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "gh api repos/{owner}/{repo} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("gh output: {e}"))
}

fn add(args: &[String]) -> i32 {
    let (Some(url), Some(why)) = (
        args.get(1).filter(|u| u.starts_with(GITHUB)),
        flag(args, "--why"),
    ) else {
        eprintln!("{USAGE}");
        return 2;
    };
    let Some((owner, repo)) = owner_repo(url) else {
        eprintln!("the url names no owner/repo");
        return 2;
    };
    let meta = match gh_repo(&owner, &repo) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let tags = flag(args, "--tags").map(|t| {
        t.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    });
    let link = from_repo(url, &why, tags, &meta, &today());
    if let Some(r) = refuse(&link) {
        eprintln!("refused: {r}");
        return 2;
    }
    let result = remote(args).and_then(|remote| {
        let mut links = remote.read()?;
        let replaced = upsert(&mut links, link.clone());
        remote.write(&links).map(|()| (replaced, links.len()))
    });
    match result {
        Ok((replaced, n)) => {
            println!(
                "{} {} ({n} links)",
                if replaced { "updated" } else { "added" },
                link.title
            );
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

fn list(args: &[String]) -> i32 {
    match remote(args).and_then(|r| r.read()) {
        Ok(links) => {
            for l in &links {
                println!("{}\t{}\t{}\t{}", l.added, l.title, l.tags.join(","), l.url);
            }
            eprintln!("{} links", links.len());
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

fn remove(args: &[String]) -> i32 {
    let Some(url) = args.get(1).map(|u| u.trim_end_matches('/').to_string()) else {
        eprintln!("{USAGE}");
        return 2;
    };
    let result = remote(args).and_then(|remote| {
        let mut links = remote.read()?;
        if !remove_url(&mut links, &url) {
            return Err(format!("no link {url}"));
        }
        remote.write(&links).map(|()| links.len())
    });
    match result {
        Ok(n) => {
            println!("removed {url} ({n} links)");
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn today_is_a_date() {
        let t = today();
        assert_eq!(t.len(), 10);
        assert!(t.starts_with("20"));
    }

    #[test]
    fn flags_read_their_value() {
        let args: Vec<String> = ["add", "u", "--why", "w", "--tags", "a,b"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(flag(&args, "--why").as_deref(), Some("w"));
        assert_eq!(flag(&args, "--tags").as_deref(), Some("a,b"));
        assert_eq!(flag(&args, "--host"), None);
    }

    fn argv(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_ssh_target_has_no_default_host_and_flags_beat_the_environment() {
        let none = |_: &str| None;
        let err = Remote::from(&argv(&["list"]), none).err().unwrap();
        assert!(err.contains("GITHUB_FINDS_HOST"), "{err}");
        let env = |v: &str| match v {
            "GITHUB_FINDS_HOST" => Some("me@ha".to_string()),
            "GITHUB_FINDS_DIR" => Some("/addon_configs/local_github_finds/".to_string()),
            "GITHUB_FINDS_KEY" => Some(" ".to_string()),
            _ => None,
        };
        let r = Remote::from(&argv(&["list"]), env).unwrap();
        assert_eq!(
            (r.host.as_str(), r.port.as_str(), r.dir.as_str(), r.key),
            ("me@ha", "22", "/addon_configs/local_github_finds", None)
        );
        let r = Remote::from(
            &argv(&[
                "list",
                "--host",
                "root@h",
                "--ssh-port",
                "2200",
                "--key",
                "k",
            ]),
            env,
        )
        .unwrap();
        assert_eq!(
            (r.host.as_str(), r.port.as_str(), r.key.as_deref()),
            ("root@h", "2200", Some("k"))
        );
        let r = Remote::from(&argv(&["list", "--host", "h"]), none).unwrap();
        assert_eq!(r.dir, DIR);
    }

    #[test]
    fn a_folder_or_port_that_could_change_the_remote_command_is_refused() {
        let none = |_: &str| None;
        for dir in [
            "relative", "/", "/a b", "/a;rm", "/a/../b", "/a'b", "/$HOME",
        ] {
            let args = argv(&["list", "--host", "h", "--dir", dir]);
            assert!(Remote::from(&args, none).is_err(), "{dir}");
        }
        let args = argv(&["list", "--host", "h", "--ssh-port", "22;x"]);
        assert!(Remote::from(&args, none).is_err());
    }
}
