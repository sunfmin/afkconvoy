//! Opening the panel of a fleet instance that is already running:
//! `afkconvoy panel --open`, which asks the host runner on its socket naming
//! its checkout, and the host runner's end, which opens the panel that fleet
//! instance holds (`serve::Live`). Nothing here reads the fleet.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::git::Repo;
use crate::runner_socket::{self, Reached, Request};
use crate::serve::{self, Live};
use crate::wake::Waker;

/// How long `panel --open` waits for the host runner's answer, which opens
/// the panel from a thread of its own, whatever the fleet instance's tick is
/// doing: so this is never a tick.
const ANSWERS_WITHIN: Duration = Duration::from_secs(10);

/// One fleet instance's panel, as the host runner opens it when asked: the
/// instance offers it once it holds it, before its first tick. A clone is the
/// same.
#[derive(Clone, Default)]
pub struct Opener {
    offered: Arc<Mutex<Option<Offered>>>,
}

/// What a fleet instance offered: its panel, its id, where it says the
/// address it opens, and how it is woken.
struct Offered {
    live: Live,
    instance: String,
    say: Box<dyn Fn(&str) + Send>,
    waker: Waker,
}

impl Opener {
    /// Has `live`, the panel of the fleet instance `instance`, opened for
    /// whoever asks from now on: the address it opens is told to `say`, and
    /// the instance woken by `waker`, to fill a page no tick was read for.
    pub fn offer(&self, live: Live, instance: &str, say: Box<dyn Fn(&str) + Send>, waker: Waker) {
        let offered = Offered { live, instance: instance.to_string(), say, waker };
        *self.offered.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(offered);
    }

    /// Opens the panel on `port`, or any free one for 0 → `panel --open`'s
    /// answer. One at a time: whoever asks second is told what the first
    /// opened. A fleet instance that has not offered its panel yet is
    /// starting, and is asked again later.
    pub fn open(&self, port: u16) -> Result<Value> {
        let offered = self.offered.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(Offered { live, instance, say, waker }) = offered.as_ref() else {
            bail!(
                "its fleet instance is starting, and holds no panel yet: ask again once its first tick ended"
            );
        };
        let (answer, began) = opened(live, instance, port)?;
        if began {
            // The address, as a fleet instance started with `--panel` says
            // it; and a tick now, to fill a page no tick was read for yet.
            say(&format!("panel: {}", answer["panel"].as_str().unwrap_or_default()));
            waker.wake();
        }
        Ok(answer)
    }
}

/// Opens `live` on `port` → the answer, and whether this began serving it.
fn opened(live: &Live, instance: &str, port: u16) -> Result<(Value, bool)> {
    let (port, began) = live.open(port)?;
    let answer = json!({"panel": serve::address(port), "port": port, "opened": began,
        "instance": instance});
    Ok((answer, began))
}

/// `afkconvoy panel --open`: has the fleet instance of the repo `repo` is a
/// checkout of, which this host's runner holds, serve its panel — on `port`,
/// or any free one — and says where. One that serves it already says the
/// address it has, whatever `port` asks. Nothing else of that fleet instance
/// changes, and a refusal changes nothing.
pub fn open(repo: &Repo, port: Option<u16>) -> Result<Value> {
    let asked = Request::Panel { checkout: repo.dir.clone(), port: port.unwrap_or(0) };
    answered(runner_socket::ask(&asked, ANSWERS_WITHIN)?)
}

/// `open`'s answer, from how asking the host runner came out.
fn answered(reached: Reached) -> Result<Value> {
    let unanswered = "the host runner did not answer, and works on as it was; whether the panel \
                      was opened is not known: ask again";
    let mut answer = match reached {
        Reached::Answer(answer) => answer,
        Reached::Nobody => bail!(
            "no host runner runs on this host, so there is no live panel to open: start one with \
             `afkconvoy run`, or print the panel once with `afkconvoy panel`"
        ),
        Reached::Closed | Reached::Unanswered => bail!(unanswered),
    };
    if let Some(error) = answer["error"].as_str() {
        bail!("the panel was not opened, and nothing changed: {error}");
    }
    let answer = answer.as_object_mut().context(unanswered)?;
    answer.remove("host_runner");
    Ok(Value::Object(answer.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wake::Wakes;

    #[test]
    fn what_is_asked_opens_the_panel_once() {
        let live = Live::new(String::new());
        let (first, began) = opened(&live, "alpha", 0).unwrap();
        assert!(began && first["opened"] == true && first["instance"] == "alpha", "{first}");
        assert_eq!(first["panel"], serve::address(first["port"].as_u64().unwrap() as u16));
        let (second, began) = opened(&live, "alpha", 0).unwrap();
        assert!(!began && second["opened"] == false);
        assert_eq!(second["panel"], first["panel"]);
    }

    #[test]
    fn an_offered_panel_is_opened_said_and_its_instance_woken_once() {
        let opener = Opener::default();
        let error = format!("{:#}", opener.open(0).err().unwrap());
        assert!(error.starts_with("its fleet instance is starting"), "{error}");

        let (wakes, said) = (Wakes::default(), Arc::new(Mutex::new(vec![])));
        let saying = Arc::clone(&said);
        let say = Box::new(move |line: &str| saying.lock().unwrap().push(line.to_string()));
        opener.offer(Live::new(String::new()), "alpha", say, wakes.waker());
        let first = opener.open(0).unwrap();
        assert_eq!(first["opened"], true, "{first}");
        let panel = first["panel"].as_str().unwrap();
        assert_eq!(*said.lock().unwrap(), [format!("panel: {panel}")]);
        assert!(wakes.rest(Duration::ZERO).is_some(), "woken to fill the page");

        let again = opener.clone().open(7070).unwrap();
        assert_eq!((&again["opened"], &again["panel"]), (&json!(false), &first["panel"]));
        assert_eq!(said.lock().unwrap().len(), 1, "said once");
        assert!(wakes.rest(Duration::ZERO).is_none(), "woken once");
    }

    #[test]
    fn an_asker_says_what_went_wrong_whoever_answered() {
        let error = format!("{:#}", answered(Reached::Nobody).err().unwrap());
        assert!(error.starts_with("no host runner runs on this host"), "{error}");
        for reached in [Reached::Closed, Reached::Unanswered] {
            let error = format!("{:#}", answered(reached).err().unwrap());
            assert!(error.contains("did not answer, and works on as it was"), "{error}");
        }
        let refused = json!({"error": "the panel cannot be served on port 80",
            "host_runner": {"pid": 42}});
        let error = format!("{:#}", answered(Reached::Answer(refused)).err().unwrap());
        assert_eq!(
            error,
            "the panel was not opened, and nothing changed: the panel cannot be served on port \
             80"
        );
        let opened = json!({"panel": "http://127.0.0.1:7070/", "port": 7070, "opened": true,
            "instance": "alpha", "host_runner": {"pid": 42}});
        let answer = answered(Reached::Answer(opened)).unwrap();
        assert_eq!(
            answer,
            json!({"panel": "http://127.0.0.1:7070/", "port": 7070, "opened": true,
                "instance": "alpha"})
        );
    }
}
