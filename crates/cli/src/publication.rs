//! Conditional publication of base, candidate state AND the unchanged source.
//!
//! `git push` omits an old==new ref, even with --atomic or a lease. Its
//! advertisement-to-transaction gap can therefore land a stale source head.
//! Send receive-pack's explicit verification command instead. Git still
//! constructs the pack, and local git or existing SSH authenticates transport;
//! no credential is read or handled here. All three refs must report success.
use std::collections::BTreeMap;
use std::io::{BufReader, Read, Write};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail, ensure};

use crate::git::Repo;

pub(crate) struct Update<'a> {
    pub name: &'a str,
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

/// Local remotes use receive-pack directly. SSH remotes retain their target;
/// GitHub HTTPS origins use the same repo through existing Git SSH login.
/// Unsupported transports are refused before commands are sent.
fn transport(repo: &Repo) -> Result<Command> {
    let url = repo.git(&["remote", "get-url", "origin"])?;
    if !url.contains("://") && !url.contains(':') || url.starts_with("file://") {
        let path = url.strip_prefix("file://").unwrap_or(&url);
        let mut command = Command::new("git");
        command.args(["receive-pack", path]).current_dir(&repo.dir);
        return Ok(command);
    }
    let (target, path, port) = if let Some(path) = url.strip_prefix("https://github.com/") {
        ("git@github.com".to_string(), path.to_string(), None)
    } else if let Some(rest) = url.strip_prefix("ssh://") {
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
            "conditional publication requires a local or SSH Git remote; configure origin as an SSH URL and verify `ssh -T <git-host>` before running land again"
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
        writeln!(input, "^{}", update.old)?;
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

pub(crate) fn push(repo: &Repo, updates: &[Update<'_>]) -> Result<bool> {
    let bytes = pack(repo, updates)?;
    let mut command = transport(repo)?;
    command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit());
    let mut child = crate::process::spawn(&mut command)
        .context("conditional publication transport could not start")?;
    let exchange = (|| -> Result<bool> {
        let mut input = child.stdin.take().context("receive-pack has no input")?;
        let mut output = BufReader::new(child.stdout.take().context("receive-pack has no output")?);
        let mut refs = BTreeMap::new();
        let mut capabilities = String::new();
        while let Some(bytes) = packet(&mut output)? {
            let line = String::from_utf8(bytes)?;
            let (reference, caps) = line.split_once('\0').unwrap_or((&line, ""));
            if !caps.is_empty() {
                capabilities = caps.trim().to_string();
            }
            if let Some((sha, name)) = reference.trim().split_once(' ') {
                refs.insert(name.to_string(), sha.to_string());
            }
        }
        if !capabilities.split_whitespace().any(|c| c == "atomic")
            || !capabilities.split_whitespace().any(|c| c == "report-status")
        {
            input.write_all(b"0000")?;
            drop(input);
            bail!("Git remote lacks atomic receive-pack status; nothing was published");
        }
        if updates.iter().any(|u| refs.get(u.name).map(String::as_str) != Some(u.old)) {
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
