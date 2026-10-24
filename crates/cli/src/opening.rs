//! Opening the panel of a fleet instance that is already running: the socket
//! `run` listens on for it, and `afkconvoy panel --open`, which asks there.
//!
//! The socket is a file beside the wake's, in the repo's git dir, so the one
//! `run` of this host for this repo is reached from any of its worktrees and
//! no other repo's is. One line crosses it each way: the port asked for, and
//! the JSON object `panel --open` prints. What is opened is the panel that
//! `run` already holds (`serve::Live`): nothing here reads the fleet.

use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::git::Repo;
use crate::host_settings;
use crate::serve::{self, Live};
use crate::wake::{Waker, home, reached};

/// The name the `run` of a repo is asked under, in the repo's git dir.
const SOCKET: &str = "convoy-panel.sock";

/// How long either end waits for the other's line. A `run` answers from a
/// thread of its own, whatever its tick is doing, so this is never a tick.
const ANSWERS_WITHIN: Duration = Duration::from_secs(10);

/// Has this `run`, which is `instance`, open `live` for whoever asks from now
/// on, in a thread of its own: a tick is neither waited for nor interrupted.
/// The address it opens is told to `say`. Called before the first tick, and
/// after the wake's listener is open. A `run` that cannot listen is one whose
/// panel only `--panel` opens, and says so to `warn`.
pub fn listen(
    repo: &Repo,
    live: Live,
    instance: &str,
    say: Box<dyn Fn(&str) + Send>,
    warn: &dyn Fn(&str),
) {
    let waker = Waker::of(repo);
    let listener = home(repo).and_then(|dir| {
        reached(&dir, SOCKET, |path| {
            // What a `run` that ended left: nobody listens on it any more.
            let _ = std::fs::remove_file(path);
            UnixListener::bind(path)
        })
    });
    let listener = match listener {
        Ok(listener) => listener,
        Err(error) => {
            warn(&format!(
                "`afkconvoy panel --open` cannot reach this fleet instance, whose panel is \
                 opened by `--panel` alone: {error:#}"
            ));
            return;
        }
    };
    let instance = instance.to_string();
    std::thread::spawn(move || {
        // One at a time: whoever asks second is told what the first opened.
        for stream in listener.incoming().flatten() {
            let _ = answer(stream, &live, &instance, waker.as_ref(), &say);
        }
    });
}

/// Answers one `panel --open`. Whatever goes wrong with it is its own loss.
fn answer(
    mut stream: UnixStream,
    live: &Live,
    instance: &str,
    waker: Option<&Waker>,
    say: &dyn Fn(&str),
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(ANSWERS_WITHIN))?;
    stream.set_write_timeout(Some(ANSWERS_WITHIN))?;
    let mut asked = String::new();
    BufReader::new(stream.try_clone()?).read_line(&mut asked)?;
    let answer = match opened(live, instance, asked.trim()) {
        Ok((answer, began)) => {
            if began {
                // The address, as a `run` started with `--panel` says it; and
                // a tick now, to fill a page no tick was read for yet.
                say(&format!("panel: {}", answer["panel"].as_str().unwrap_or_default()));
                if let Some(waker) = waker {
                    waker.wake();
                }
            }
            answer
        }
        Err(error) => json!({"error": format!("{error:#}")}),
    };
    writeln!(stream, "{answer}")
}

/// Opens `live` on the port `asked` names → the answer, and whether this
/// began serving it.
fn opened(live: &Live, instance: &str, asked: &str) -> Result<(Value, bool)> {
    let port: u16 = asked.parse().with_context(|| format!("{asked:?} is not a port"))?;
    let (port, began) = live.open(port)?;
    let answer = json!({"panel": serve::address(port), "port": port, "opened": began,
        "instance": instance});
    Ok((answer, began))
}

/// `afkconvoy panel --open`: has the `run` of this host for the repo `repo` is
/// a checkout of serve its panel — on `port`; where none is named, on the
/// panel's port of the host settings, or any free one where they say none or
/// it is taken — and says where. A `run` that serves it already says the
/// address it has, whatever `port` asks. Nothing else of that `run` changes,
/// and a refusal changes nothing.
pub fn open(repo: &Repo, port: Option<u16>) -> Result<Value> {
    let dir = home(repo)?;
    if let Some(port) = port {
        return ask(&dir, port);
    }
    match host_settings::of_host()?.panel {
        Some(port) => ask(&dir, port).or_else(|_| ask(&dir, 0)),
        None => ask(&dir, 0),
    }
}

fn ask(dir: &Path, port: u16) -> Result<Value> {
    let mut stream = match reached(dir, SOCKET, |path| UnixStream::connect(path)) {
        Ok(stream) => stream,
        Err(error) => {
            let kind = error.downcast_ref::<std::io::Error>().map(std::io::Error::kind);
            if matches!(kind, Some(ErrorKind::NotFound | ErrorKind::ConnectionRefused)) {
                bail!(
                    "no afkconvoy run is running for this repo on this host, so there is no \
                     live panel to open: start one with `afkconvoy run`, or print the panel \
                     once with `afkconvoy panel`"
                );
            }
            return Err(error.context("the run of this repo on this host could not be asked"));
        }
    };
    let unanswered = "the run of this repo on this host did not answer, and works on as it \
                      was; whether its panel was opened is not known: ask again";
    let mut line = String::new();
    let answered = stream
        .set_read_timeout(Some(ANSWERS_WITHIN))
        .and_then(|()| stream.set_write_timeout(Some(ANSWERS_WITHIN)))
        .and_then(|()| writeln!(stream, "{port}"))
        .and_then(|()| BufReader::new(&stream).read_line(&mut line));
    answered.context(unanswered)?;
    let answer: Value = serde_json::from_str(line.trim()).context(unanswered)?;
    if let Some(error) = answer["error"].as_str() {
        bail!("the panel was not opened, and the run works on as it was: {error}");
    }
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Listens where a `run` would in `dir`, reached as a `run` reaches it: a
    /// temporary directory's path may be too long for a socket's.
    fn bind(dir: &Path) -> UnixListener {
        reached(dir, SOCKET, |path| UnixListener::bind(path)).unwrap()
    }

    /// A git dir with something other than a `run` listening in it, which
    /// answers each asker with `line`.
    fn stand_in(line: &'static str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let listener = bind(dir.path());
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let _ = BufReader::new(&stream).read_line(&mut String::new());
                let _ = stream.write_all(line.as_bytes());
            }
        });
        dir
    }

    #[test]
    fn what_is_asked_opens_the_panel_once_and_a_port_that_is_none_opens_nothing() {
        let live = Live::new(String::new());
        let error = opened(&live, "alpha", "seventy").err().unwrap();
        assert!(format!("{error:#}").contains("\"seventy\" is not a port"), "{error:#}");
        assert!(!live.served());
        let (first, began) = opened(&live, "alpha", "0").unwrap();
        assert!(began && first["opened"] == true && first["instance"] == "alpha", "{first}");
        assert_eq!(first["panel"], serve::address(first["port"].as_u64().unwrap() as u16));
        let (second, began) = opened(&live, "alpha", "0").unwrap();
        assert!(!began && second["opened"] == false);
        assert_eq!(second["panel"], first["panel"]);
    }

    #[test]
    fn an_asker_says_what_went_wrong_whoever_is_at_the_socket() {
        let nobody = tempfile::tempdir().unwrap();
        let error = format!("{:#}", ask(nobody.path(), 0).err().unwrap());
        assert!(error.starts_with("no afkconvoy run is running for this repo"), "{error}");

        // a `run` that ended left its socket: nobody listens on it
        drop(bind(nobody.path()));
        let error = format!("{:#}", ask(nobody.path(), 0).err().unwrap());
        assert!(error.starts_with("no afkconvoy run is running for this repo"), "{error}");

        for line in ["", "not json\n"] {
            let error = format!("{:#}", ask(stand_in(line).path(), 0).err().unwrap());
            assert!(error.contains("did not answer, and works on as it was"), "{line:?}: {error}");
        }
        let refused = stand_in("{\"error\":\"the panel cannot be served on port 80\"}\n");
        let error = format!("{:#}", ask(refused.path(), 80).err().unwrap());
        assert_eq!(
            error,
            "the panel was not opened, and the run works on as it was: the panel cannot be \
             served on port 80"
        );
        let answered = stand_in("{\"panel\":\"http://127.0.0.1:7070/\"}\n");
        assert_eq!(ask(answered.path(), 0).unwrap()["panel"], "http://127.0.0.1:7070/");
    }
}
