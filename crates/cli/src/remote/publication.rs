//! Conditional publication of base, candidate state AND the unchanged source.
//!
//! `git push` omits an old==new ref, even with --atomic or a lease. Its
//! advertisement-to-transaction gap can therefore land a stale source head.
//! Local and SSH remotes are sent receive-pack's explicit verification
//! command instead; git still constructs the pack. A GitHub HTTPS origin is
//! sent GitHub's `updateRefs`, which checks every ref's old value and applies
//! all or none, after a plain `git push` has carried the objects to scratch
//! refs the same call deletes. git's own credential helper and `gh`
//! authenticate; no credential is read or handled here. All refs must take.
use std::collections::BTreeMap;
use std::io::{BufReader, Read, Write};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail, ensure};

use crate::git::Repo;

mod github;

pub(crate) struct Update<'a> {
    pub name: &'a str,
    /// Expected old object, or the object-format's zero ID to create a ref.
    pub old: &'a str,
    pub new: &'a str,
}

fn packet(reader: &mut impl Read) -> Result<Option<Vec<u8>>> {
    let mut size = [0; 4];
    reader
        .read_exact(&mut size)
        .context("receive-pack advertisement/status ended early; check Git SSH authentication")?;
    let n = usize::from_str_radix(std::str::from_utf8(&size)?, 16)?;
    if n == 0 {
        return Ok(None);
    }
    ensure!((4..=65520).contains(&n), "receive-pack sent an invalid packet length");
    let mut bytes = vec![0; n - 4];
    reader.read_exact(&mut bytes)?;
    Ok(Some(bytes))
}

fn send(writer: &mut impl Write, line: &str) -> Result<()> {
    ensure!(line.len() <= 65516, "publication ref command is too long");
    write!(writer, "{:04x}", line.len() + 4)?;
    writer.write_all(line.as_bytes())?;
    Ok(())
}

/// How a publication reaches origin's push URL.
enum Route {
    /// receive-pack, run locally or over SSH.
    ReceivePack(Command),
    /// A GitHub HTTPS origin: `git push` for the objects, `updateRefs` for
    /// the refs. The repo is `owner/name`.
    GitHub(String),
}

/// Local remotes use receive-pack directly. SSH remotes retain their target.
/// GitHub HTTPS origins go through git's credential helper and `gh`.
/// Unsupported transports are refused before commands are sent.
fn route(repo: &Repo) -> Result<Route> {
    let urls = repo.git(&["remote", "get-url", "--push", "--all", "origin"])?;
    let urls: Vec<_> = urls.lines().collect();
    ensure!(
        urls.len() == 1,
        "conditional publication requires exactly one origin push URL; configure one push destination before running land"
    );
    let url = urls[0];
    if let Some(repo) = github_https(url) {
        return Ok(Route::GitHub(repo));
    }
    transport(repo, url).map(Route::ReceivePack)
}

/// `owner/name` of a GitHub HTTPS URL, with or without `.git`.
fn github_https(url: &str) -> Option<String> {
    let path = url.strip_prefix("https://github.com/")?.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, name) = path.split_once('/')?;
    let fine = |part: &str| {
        !part.is_empty()
            && part.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
    };
    (fine(owner) && fine(name)).then(|| format!("{owner}/{name}"))
}

fn transport(repo: &Repo, url: &str) -> Result<Command> {
    if !url.contains("://") && !url.contains(':') || url.starts_with("file://") {
        let path = url.strip_prefix("file://").unwrap_or(url);
        let mut command = Command::new("git");
        command.args(["receive-pack", path]).current_dir(&repo.dir);
        return Ok(command);
    }
    let (target, path, port) = if let Some(rest) = url.strip_prefix("ssh://") {
        let (host, path) = rest.split_once('/').context("SSH origin has no repo path")?;
        let (host, port) = match host.rsplit_once(':') {
            Some((host, port)) => (host, Some(port.to_string())),
            None => (host, None),
        };
        (host.to_string(), format!("/{path}"), port)
    } else if let Some((host, path)) = url.split_once(':').filter(|(host, _)| !host.contains('/')) {
        (host.to_string(), path.to_string(), None)
    } else {
        bail!(
            "conditional publication requires a local, SSH or GitHub HTTPS Git remote; configure origin as one before running land again"
        );
    };
    ensure!(!target.starts_with('-') && !path.is_empty(), "invalid Git SSH origin");
    let remote = afkconvoy_core::line::Line::new().fixed("git-receive-pack ").word(&path).own();
    let mut command = Command::new("ssh");
    command.args([
        "-o",
        "BatchMode=yes",
        "-o",
        "StrictHostKeyChecking=yes",
        "-o",
        "ConnectTimeout=10",
        "-o",
        "ServerAliveInterval=10",
        "-o",
        "ServerAliveCountMax=2",
    ]);
    if let Some(port) = port {
        command.args(["-p", &port]);
    }
    command.arg(target).arg(remote);
    Ok(command)
}

fn pack(repo: &Repo, updates: &[Update<'_>]) -> Result<Vec<u8>> {
    let mut command = Command::new("git");
    command
        .args(["pack-objects", "--revs", "--stdout"])
        .current_dir(&repo.dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = crate::process::spawn(&mut command)?;
    let mut input = child.stdin.take().context("pack-objects has no input")?;
    for update in updates {
        writeln!(input, "{}", update.new)?;
    }
    for update in updates {
        if !update.old.bytes().all(|c| c == b'0') {
            writeln!(input, "^{}", update.old)?;
        }
    }
    drop(input);
    let output = child.wait_with_output()?;
    ensure!(
        output.status.success(),
        "cannot construct publication pack: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output.stdout)
}

fn advertisement(reader: &mut impl Read) -> Result<(BTreeMap<String, String>, bool)> {
    let mut refs = BTreeMap::new();
    let mut capabilities = String::new();
    while let Some(bytes) = packet(reader)? {
        let line = String::from_utf8(bytes)?;
        let (reference, caps) = line.split_once('\0').unwrap_or((&line, ""));
        if !caps.is_empty() {
            capabilities = caps.trim().to_string();
        }
        if let Some((sha, name)) = reference.trim().split_once(' ') {
            refs.insert(name.to_string(), sha.to_string());
        }
    }
    let supports = capabilities.split_whitespace().any(|c| c == "atomic")
        && capabilities.split_whitespace().any(|c| c == "report-status");
    Ok((refs, supports))
}

/// Startup checks authentication and atomic capability without sending any
/// update command. This is safe alongside another publishing turn.
pub(crate) fn check(repo: &Repo) -> Result<()> {
    let mut command = match route(repo)? {
        Route::ReceivePack(command) => command,
        Route::GitHub(_) => return github::check(repo),
    };
    command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit());
    let mut child = crate::process::spawn(&mut command)?;
    let exchange = (|| -> Result<()> {
        let mut input = child.stdin.take().context("receive-pack has no input")?;
        let mut output = BufReader::new(child.stdout.take().context("receive-pack has no output")?);
        let (_, supports) = advertisement(&mut output)?;
        input.write_all(b"0000")?;
        drop(input);
        let mut rest = Vec::new();
        output.read_to_end(&mut rest)?;
        ensure!(
            supports,
            "Git remote lacks atomic receive-pack status; this host cannot publish candidates"
        );
        Ok(())
    })();
    if exchange.is_err() {
        let _ = child.kill();
    }
    let status = child.wait()?;
    exchange?;
    ensure!(
        status.success(),
        "conditional publication authentication failed; verify Git SSH access before starting this fleet"
    );
    Ok(())
}

pub(crate) fn push(repo: &Repo, updates: &[Update<'_>]) -> Result<bool> {
    let mut command = match route(repo)? {
        Route::ReceivePack(command) => command,
        Route::GitHub(name) => return github::publish(repo, &name, updates),
    };
    let bytes = pack(repo, updates)?;
    command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit());
    let mut child = crate::process::spawn(&mut command)
        .context("conditional publication transport could not start")?;
    let exchange = (|| -> Result<bool> {
        let mut input = child.stdin.take().context("receive-pack has no input")?;
        let mut output = BufReader::new(child.stdout.take().context("receive-pack has no output")?);
        let (refs, supports) = advertisement(&mut output)?;
        if !supports {
            input.write_all(b"0000")?;
            drop(input);
            bail!("Git remote lacks atomic receive-pack status; nothing was published");
        }
        if updates.iter().any(|u| {
            let expected = (!u.old.bytes().all(|c| c == b'0')).then_some(u.old);
            refs.get(u.name).map(String::as_str) != expected
        }) {
            input.write_all(b"0000")?;
            drop(input);
            return Ok(false);
        }
        for (i, update) in updates.iter().enumerate() {
            let caps = if i == 0 { "\0report-status atomic" } else { "" };
            send(&mut input, &format!("{} {} {}{caps}\n", update.old, update.new, update.name))?;
        }
        input.write_all(b"0000")?;
        input.write_all(&bytes)?;
        drop(input);
        let mut unpacked = false;
        let mut accepted = vec![];
        while let Some(bytes) = packet(&mut output)? {
            let line = String::from_utf8(bytes)?;
            if line.trim() == "unpack ok" {
                unpacked = true;
            }
            if let Some(name) = line.trim().strip_prefix("ok ") {
                accepted.push(name.to_string());
            }
            if line.starts_with("ng ") || line.starts_with("unpack ") && !unpacked {
                eprintln!("[afkconvoy] publication: {}", line.trim());
            }
        }
        let mut rest = Vec::new();
        output.read_to_end(&mut rest)?;
        Ok(unpacked && updates.iter().all(|u| accepted.iter().any(|name| name == u.name)))
    })();
    if exchange.is_err() {
        let _ = child.kill();
    }
    let status = child.wait()?;
    Ok(exchange? && status.success())
}
