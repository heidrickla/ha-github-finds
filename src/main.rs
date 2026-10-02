//! `github-finds serve` runs in the add-on; `add`, `list` and `remove` edit the links file on the
//! Home Assistant host over SSH, taking a repository's details from `gh api`.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use github_finds::{
    GITHUB, Link, date_from_days, from_repo, owner_repo, parse, refuse, remove as remove_url,
    respond, upsert,
};
use tiny_http::{Header, Method, Response, Server};

const USAGE: &str = "usage:
  github-finds serve [--links /config/links.json] [--port 8091]
  github-finds add <github url> --why <text> [--tags a,b] [--host root@192.0.2.10] [--ssh-port 22] [--key ~/.ssh/ha_ed25519]
  github-finds list | remove <github url>   (same --host, --ssh-port, --key)";
const DIR: &str = "/addon_configs/local_github_finds";

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("serve") => serve(&args),
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
            respond(request.url(), file.as_deref())
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

struct Remote {
    host: String,
    port: String,
    key: String,
}

impl Remote {
    fn from(args: &[String]) -> Remote {
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_default();
        Remote {
            host: flag(args, "--host").unwrap_or_else(|| "root@192.0.2.10".into()),
            port: flag(args, "--ssh-port").unwrap_or_else(|| "22".into()),
            key: flag(args, "--key").unwrap_or_else(|| format!("{home}/.ssh/ha_ed25519")),
        }
    }

    fn run(&self, command: &str, input: Option<&[u8]>) -> Result<String, String> {
        let mut child = Command::new("ssh")
            .args([
                "-i",
                &self.key,
                "-p",
                &self.port,
                "-o",
                "BatchMode=yes",
                &self.host,
                command,
            ])
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
        let out = self.run(&format!("echo __FINDS__; sudo -n cat {DIR}/links.json 2>/dev/null || echo '[]'; echo __END__"), None)?;
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
        self.run(
            &format!("sudo -n sh -c 'mkdir -p {DIR} && cat > {DIR}/.links.tmp && chmod 644 {DIR}/.links.tmp && mv {DIR}/.links.tmp {DIR}/links.json'"),
            Some(json.as_bytes()),
        )
        .map(|_| ())
    }
}

/// Today's UTC date as YYYY-MM-DD.
fn today() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    date_from_days(i64::try_from(secs / 86_400).unwrap_or(0))
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
    let remote = Remote::from(args);
    let result = remote.read().and_then(|mut links| {
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
    match Remote::from(args).read() {
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
    let remote = Remote::from(args);
    let result = remote.read().and_then(|mut links| {
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
}
