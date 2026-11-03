//! Host settings: what a host says for every fleet instance on it — the
//! limits past which a worker is hot, the shares admission holds back at, the
//! panel's port (docs/adr/0023). They are kept in the checkout list's file,
//! beside the checkouts, and changed by any `run` that names one: that `run`
//! writes them, and every fleet instance reads them where it uses them —
//! admission at each tick, the hot limits at each sample — so a change holds
//! for every one at once, and none is started again for it. The panel's port alone is read
//! once, by the host runner as it starts. A `run` names one by its flag, or
//! by the variable of its environment that stands for that flag; nothing
//! else reads those variables.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use afkconvoy_core::admission::SHARES;
use afkconvoy_core::resources::SPAN;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::checkout_list::List;
use crate::hot;

/// What a host says; each it leaves unsaid (`None`) is the default. Its keys,
/// and the units of their values, are those of the flag of `run` that names
/// each: `hot_cores` is `--hot-cores`, in cores.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// `--hot-cores`, in thousandths of a core.
    #[serde(with = "thousandths")]
    pub hot_cores: Option<u64>,
    /// `--hot-memory`, in thousandths of a GiB.
    #[serde(with = "thousandths")]
    pub hot_memory: Option<u64>,
    /// `--hot-written`, in thousandths of a GiB.
    #[serde(with = "thousandths")]
    pub hot_written: Option<u64>,
    /// `--admit-load`: the load admission holds back past, a whole percent
    /// of the host's cores.
    pub admit_load: Option<u64>,
    /// `--admit-memory`: the memory free admission holds back under, a whole
    /// percent of the host's.
    pub admit_memory: Option<u64>,
    /// `--admit-disk`: the disk free admission holds back under, a whole
    /// percent of the checkout's.
    pub admit_disk: Option<u64>,
    /// `--admit-inspect-after`: the seconds admission holds back for one
    /// reason before the host calls for a watch.
    pub admit_inspect_after: Option<u64>,
    /// `--exclusive-seconds`: seconds a running command may hold others up.
    pub exclusive_seconds: Option<u64>,
    /// `--gate-lanes`: worker commands that may run across different repos.
    pub gate_lanes: Option<u64>,
    /// `--slow-gate-lanes`: rounds' slow gates that may run at once, beside
    /// the worker lanes.
    pub slow_gate_lanes: Option<u64>,
    /// `--fast-lanes`: fast gates that may run at once on the host, whatever
    /// their repo, beside the worker lanes (docs/adr/0049).
    pub fast_lanes: Option<u64>,
    /// An old checkout list may still name `train_lanes`. It is not a setting:
    /// a known field so `rest` does not keep it, and left out of every write.
    #[serde(default, skip_serializing)]
    train_lanes: Option<u64>,
    /// `--panel`: the port the host runner serves the panel on; `PANEL_PORT`
    /// where the host says none.
    pub panel: Option<u16>,
    /// `--watch-dir`: the operator's directory; none uses the stock watch's.
    pub watch_dir: Option<PathBuf>,
    /// `--watch-every`: seconds between the host runner's looks.
    pub watch_every: Option<u64>,
    /// `--jev-key-command`: a shell command whose stdout is the TypeSafe key.
    pub jev_key_command: Option<String>,
    /// `--notify-command`: receives one human decision as JSON on stdin.
    pub notify_command: Option<String>,
    /// `--host-name`: the name this host goes by in every record it owns, so
    /// that its records stay its own when the network renames it. A `run`
    /// that finds none keeps the name the host has then (`pin_host_name`).
    pub host_name: Option<String>,
    /// `--agent-instructions`: the agent setup's instructions file, linked to
    /// every held harness's global instructions (sunfmin/control#528).
    pub agent_instructions: Option<PathBuf>,
    /// `--agent-skills`: the agent setup's skill list, installed for every
    /// held harness.
    pub agent_skills: Option<PathBuf>,
    /// What another afkconvoy says beside them, kept as it is: an older one's
    /// `no_host_sleep` among them, which keeps that one from putting the
    /// host to sleep (docs/adr/0050).
    #[serde(flatten)]
    rest: Map<String, Value>,
}

/// The port the host runner serves the panel on where the host settings say
/// none.
pub const PANEL_PORT: u16 = 7070;
/// How often the host runner looks at its host where it says no interval.
pub const WATCH_EVERY: u64 = 300;
/// The default number of worker lanes on a host.
pub const GATE_LANES: u64 = 1;
/// The default number of fast gates that run at once on a host.
pub const FAST_LANES: u64 = 2;

/// What a `run` named of the host settings; each it left unnamed is `None`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Named {
    pub hot_cores: Option<To<u64>>,
    pub hot_memory: Option<To<u64>>,
    pub hot_written: Option<To<u64>>,
    pub admit_load: Option<To<u64>>,
    pub admit_memory: Option<To<u64>>,
    pub admit_disk: Option<To<u64>>,
    pub admit_inspect_after: Option<To<u64>>,
    pub exclusive_seconds: Option<To<u64>>,
    pub gate_lanes: Option<To<u64>>,
    pub slow_gate_lanes: Option<To<u64>>,
    pub fast_lanes: Option<To<u64>>,
    pub panel: Option<To<u16>>,
    pub watch_dir: Option<To<PathBuf>>,
    pub watch_every: Option<To<u64>>,
    pub jev_key_command: Option<To<String>>,
    pub notify_command: Option<To<String>>,
    pub host_name: Option<To<String>>,
    pub agent_instructions: Option<To<PathBuf>>,
    pub agent_skills: Option<To<PathBuf>>,
}

/// What a `run` sets one setting to: a value of its own, or back to its
/// default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum To<T> {
    Default,
    Value(T),
}

impl<T> To<T> {
    /// As a setting keeps it: the default is unsaid.
    pub fn kept(self) -> Option<T> {
        match self {
            To::Default => None,
            To::Value(value) => Some(value),
        }
    }

    /// The word that sets a setting back to its default, or `value` parsed
    /// as `parse` reads it.
    pub fn parse(said: &str, parse: impl Fn(&str) -> Result<T, String>) -> Result<To<T>, String> {
        match said {
            "default" => Ok(To::Default),
            said => parse(said).map(To::Value),
        }
    }
}

/// `--hot-cores`, `--hot-memory` and `--hot-written` as said: a number of
/// cores or GiB above zero, whole or not, as thousandths of it; or `default`.
pub fn thousandths_or_default(said: &str) -> Result<To<u64>, String> {
    To::parse(said, |said| match said.parse::<f64>().ok().and_then(thousandths::of) {
        Some(thousandths) => Ok(thousandths),
        None => Err(format!("`{said}` is not a number above zero, nor `default`")),
    })
}

/// `--admit-*` as said: a whole number, or `default`.
pub fn whole_or_default(said: &str) -> Result<To<u64>, String> {
    To::parse(said, |said| {
        said.parse().map_err(|_| format!("`{said}` is not a whole number, nor `default`"))
    })
}

/// `--panel` as said: a port, or `default`.
pub fn port_or_default(said: &str) -> Result<To<u16>, String> {
    To::parse(said, |said| said.parse().map_err(|_| format!("`{said}` is no port, nor `default`")))
}

/// `--watch-dir` as said: the operator's directory, an existing one, kept by
/// its whole path, or `default` for the stock watch's own directory.
pub fn watch_dir_or_default(said: &str) -> Result<To<PathBuf>, String> {
    To::parse(said, |said| {
        std::fs::canonicalize(said)
            .ok()
            .filter(|dir| dir.is_dir())
            .ok_or_else(|| format!("`{said}` is no directory, nor `default`"))
    })
}

/// `--agent-instructions` and `--agent-skills` as said: an existing file,
/// kept by its whole path, or `default` for none.
pub fn file_or_default(said: &str) -> Result<To<PathBuf>, String> {
    To::parse(said, |said| {
        std::fs::canonicalize(said)
            .ok()
            .filter(|file| file.is_file())
            .ok_or_else(|| format!("`{said}` is no file, nor `default`"))
    })
}

/// `--watch-every` as said: seconds above zero, or `default`.
pub fn watch_every_or_default(said: &str) -> Result<To<u64>, String> {
    To::parse(said, |said| {
        said.parse::<u64>()
            .ok()
            .filter(|seconds| *seconds > 0)
            .ok_or_else(|| format!("`{said}` is not a whole number of at least 1, nor `default`"))
    })
}

/// `--jev-key-command` as said: a nonempty shell command, or `default`.
pub fn jev_key_command_or_default(said: &str) -> Result<To<String>, String> {
    To::parse(said, |said| {
        if said.trim().is_empty() {
            Err("the Jev key command is empty; name a command, or `default`".into())
        } else {
            Ok(said.to_string())
        }
    })
}

/// `--notify-command` as said: a nonempty shell command, or `default`.
pub fn notify_command_or_default(said: &str) -> Result<To<String>, String> {
    To::parse(said, |said| {
        if said.trim().is_empty() {
            Err("the notify command is empty; name a command, or `default`".into())
        } else {
            Ok(said.to_string())
        }
    })
}

impl Named {
    /// Whether it names none.
    pub fn is_empty(&self) -> bool {
        *self == Named::default()
    }
}

impl Settings {
    /// These, with what `named` names laid over them.
    pub fn with(mut self, named: &Named) -> Settings {
        fn lay<T: Clone>(kept: &mut Option<T>, named: Option<&To<T>>) {
            if let Some(to) = named {
                *kept = to.clone().kept();
            }
        }
        lay(&mut self.hot_cores, named.hot_cores.as_ref());
        lay(&mut self.hot_memory, named.hot_memory.as_ref());
        lay(&mut self.hot_written, named.hot_written.as_ref());
        lay(&mut self.admit_load, named.admit_load.as_ref());
        lay(&mut self.admit_memory, named.admit_memory.as_ref());
        lay(&mut self.admit_disk, named.admit_disk.as_ref());
        lay(&mut self.admit_inspect_after, named.admit_inspect_after.as_ref());
        lay(&mut self.exclusive_seconds, named.exclusive_seconds.as_ref());
        lay(&mut self.gate_lanes, named.gate_lanes.as_ref());
        lay(&mut self.slow_gate_lanes, named.slow_gate_lanes.as_ref());
        lay(&mut self.fast_lanes, named.fast_lanes.as_ref());
        lay(&mut self.panel, named.panel.as_ref());
        lay(&mut self.watch_dir, named.watch_dir.as_ref());
        lay(&mut self.watch_every, named.watch_every.as_ref());
        lay(&mut self.jev_key_command, named.jev_key_command.as_ref());
        lay(&mut self.notify_command, named.notify_command.as_ref());
        lay(&mut self.host_name, named.host_name.as_ref());
        lay(&mut self.agent_instructions, named.agent_instructions.as_ref());
        lay(&mut self.agent_skills, named.agent_skills.as_ref());
        self
    }

    /// Each setting, by its key, as the host says it: null where it says
    /// none, and the default holds.
    pub fn said(&self) -> Map<String, Value> {
        match serde_json::to_value(self) {
            Ok(Value::Object(said)) => said,
            _ => unreachable!("settings are a JSON object"),
        }
    }

    /// What changed from these to `now`: each setting whose value differs,
    /// by its key, as {from, to} — null being the default.
    pub fn changes(&self, now: &Settings) -> Map<String, Value> {
        let was = self.said();
        let changed = now.said().into_iter().filter_map(|(key, to)| {
            let from = was.get(&key).cloned().unwrap_or(Value::Null);
            (from != to).then(|| (key, json!({"from": from, "to": to})))
        });
        changed.collect()
    }

    /// The port it has the host runner serve the panel on.
    pub fn panel_port(&self) -> u16 {
        self.panel.unwrap_or(PANEL_PORT)
    }

    /// The exclusive bound, counting only time with another command waiting.
    pub fn exclusive_seconds(&self) -> u64 {
        self.exclusive_seconds.unwrap_or(crate::exclusive::SECONDS)
    }

    pub fn gate_lanes(&self) -> u64 {
        self.gate_lanes.unwrap_or(GATE_LANES).max(1)
    }

    /// The slow lanes, one where the host says none.
    pub fn slow_gate_lanes(&self) -> u64 {
        self.slow_gate_lanes.unwrap_or(1).max(1)
    }

    pub fn fast_lanes(&self) -> u64 {
        self.fast_lanes.unwrap_or(FAST_LANES).max(1)
    }

    /// The worker lanes, the fast lane and the slow lanes, as the queue is
    /// scheduled by them.
    pub fn lanes(&self) -> afkconvoy_core::exclusive::Lanes {
        afkconvoy_core::exclusive::Lanes {
            worker: self.gate_lanes(),
            fast: self.fast_lanes(),
            slow: self.slow_gate_lanes(),
        }
    }

    /// The share of this host's cores one gate's tests are given: its cores
    /// divided by every lane a gate may run in at once, at least one
    /// (sunfmin/control#245).
    pub fn gate_share(&self) -> u64 {
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get() as u64);
        let lanes = self.gate_lanes() + self.fast_lanes() + self.slow_gate_lanes();
        (cores / lanes).max(1)
    }

    /// The seconds between looks at the host.
    pub fn watch_every(&self) -> u64 {
        self.watch_every.unwrap_or(WATCH_EVERY)
    }

    /// The hot limits it says, as the sampler takes them.
    pub fn hot(&self) -> hot::Said {
        let bytes = |thousandths: u64| thousandths.saturating_mul(1 << 30) / 1000;
        hot::Said {
            millicores: self.hot_cores,
            resident: self.hot_memory.map(bytes),
            written: self.hot_written.map(bytes),
        }
    }

    /// Each setting in force for the fleet of the checkout at `dir`, by its
    /// key, in the units of its flag: what the host says, else its default
    /// worked out from the look's host reading.
    pub fn in_force(
        &self,
        dir: &Path,
        readings: &crate::admission::Readings,
    ) -> Map<String, Value> {
        let limits = readings.hot_limits(self.hot(), dir);
        let gib = |bytes: u64| (bytes as u128 * 1000 / (1 << 30)) as f64 / 1000.0;
        let in_force = json!({
            "hot_cores": limits.millicores as f64 / 1000.0,
            "hot_memory": gib(limits.resident),
            "hot_written": gib(limits.written),
            "admit_load": self.admit_load.unwrap_or(SHARES.load),
            "admit_memory": self.admit_memory.unwrap_or(SHARES.memory),
            "admit_disk": self.admit_disk.unwrap_or(SHARES.disk),
            "admit_inspect_after": self.admit_inspect_after.unwrap_or(SPAN),
            "exclusive_seconds": self.exclusive_seconds(),
            "gate_lanes": self.gate_lanes(),
            "slow_gate_lanes": self.slow_gate_lanes(),
            "fast_lanes": self.fast_lanes(),
            "panel": self.panel_port(),
            "watch_dir": crate::host_watch::directory(self).ok(),
            "watch_every": self.watch_every(),
            "jev_key_command": self.jev_key_command,
            "notify_command": self.notify_command,
            "host_name": self.host_name.clone().unwrap_or_else(system_host_name),
            "agent_instructions": self.agent_instructions,
            "agent_skills": self.agent_skills,
        });
        match in_force {
            Value::Object(in_force) => in_force,
            _ => unreachable!("built as an object"),
        }
    }
}

/// `--host-name` as said: a nonempty name, or `default`.
pub fn host_name_or_default(said: &str) -> Result<To<String>, String> {
    To::parse(said, |said| match said.trim() {
        "" => Err("the host name is empty; name one, or `default`".into()),
        name => Ok(name.to_string()),
    })
}

/// The name the system gives this host now, which a network may change.
pub fn system_host_name() -> String {
    gethostname::gethostname().to_string_lossy().into()
}

/// The name this host goes by in the records it owns: the one its settings
/// keep, else the system's. Read once per process.
pub fn host_name() -> String {
    static NAME: OnceLock<String> = OnceLock::new();
    let name = || of_host().ok().and_then(|settings| settings.host_name);
    NAME.get_or_init(|| name().unwrap_or_else(system_host_name)).clone()
}

/// Keeps the system's name for this host in `list`, where its settings keep
/// none: a `run` does, so that a later rename by the network leaves its
/// records its own (a reboot onto another network renamed
/// `Offices-MacBook-Pro.local` to `Offices-MBP.lan`).
pub fn pin_host_name(list: &List) -> Result<()> {
    if list.read()?.settings.host_name.is_some() {
        return Ok(());
    }
    list.change(|listed| {
        listed.settings.host_name.get_or_insert_with(system_host_name);
    })
}

/// What `run --foreground` named of the host settings: laid over the host's
/// for this process alone.
static LAID: OnceLock<Named> = OnceLock::new();

/// Lays `named` over the host's settings for this process alone, writing
/// nothing: `run --foreground`, which leaves the checkout list as it is.
pub fn lay_for_this_process(named: Named) {
    let _ = LAID.set(named);
}

/// The host settings in force in this process: the host's, as its checkout
/// list's file says them, with what this process's `run --foreground` named
/// laid over them. A file that cannot be read is an error naming it.
pub fn of_host() -> Result<Settings> {
    let kept = List::of_host()?.read()?.settings;
    Ok(match LAID.get() {
        Some(named) => kept.with(named),
        None => kept,
    })
}

/// Has the host say what `named` names, in `list` → what changed, as
/// `Settings::changes` says it. One that names nothing changes nothing and
/// writes nothing; one that names a setting as it is changes nothing either.
pub fn change(list: &List, named: &Named) -> Result<Map<String, Value>> {
    if named.is_empty() {
        return Ok(Map::new());
    }
    list.change(|listed| {
        let was = listed.settings.clone();
        listed.settings = was.clone().with(named);
        was.changes(&listed.settings)
    })
}

/// The settings of `list`, as `doctor` answers them: {file, said, in_force} —
/// what the host says (null: it says none, and the default holds), and what
/// is in force for the fleet of the checkout at `dir`; {file, error} where
/// the file cannot be read.
pub fn json(list: &List, dir: &Path, readings: &crate::admission::Readings) -> Value {
    match list.read() {
        Ok(listed) => json!({
            "file": list.file(),
            "said": listed.settings.said(),
            "in_force": listed.settings.in_force(dir, readings),
        }),
        Err(error) => json!({"file": list.file(), "error": format!("{error:#}")}),
    }
}

/// The variables of a `run`'s environment that name a host setting as its
/// flag does, where the flag is not given.
pub const HOT_CORES_ENV: &str = "AFKCONVOY_HOT_CORES";
pub const HOT_MEMORY_ENV: &str = "AFKCONVOY_HOT_MEMORY";
pub const HOT_WRITTEN_ENV: &str = "AFKCONVOY_HOT_WRITTEN";
pub const ADMIT_LOAD_ENV: &str = "AFKCONVOY_ADMIT_LOAD";
pub const ADMIT_MEMORY_ENV: &str = "AFKCONVOY_ADMIT_MEMORY";
pub const ADMIT_DISK_ENV: &str = "AFKCONVOY_ADMIT_DISK";
pub const ADMIT_INSPECT_AFTER_ENV: &str = "AFKCONVOY_ADMIT_INSPECT_AFTER";
pub const EXCLUSIVE_SECONDS_ENV: &str = "AFKCONVOY_EXCLUSIVE_SECONDS";
pub const WATCH_DIR_ENV: &str = "AFKCONVOY_WATCH_DIR";
pub const WATCH_EVERY_ENV: &str = "AFKCONVOY_WATCH_EVERY";
pub const JEV_KEY_COMMAND_ENV: &str = "AFKCONVOY_JEV_KEY_COMMAND";
pub const NOTIFY_COMMAND_ENV: &str = "AFKCONVOY_NOTIFY_COMMAND";
pub const HOST_NAME_ENV: &str = "AFKCONVOY_HOST_NAME";
pub const AGENT_INSTRUCTIONS_ENV: &str = "AFKCONVOY_AGENT_INSTRUCTIONS";
pub const AGENT_SKILLS_ENV: &str = "AFKCONVOY_AGENT_SKILLS";

/// A number of cores or GiB as the file keeps it — the number, whole or not
/// — and as a setting holds it, in thousandths of it.
mod thousandths {
    use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};

    /// `number` in thousandths: `None` where it is no number above zero.
    pub fn of(number: f64) -> Option<u64> {
        (number.is_finite() && number > 0.0).then(|| (number * 1000.0).round() as u64)
    }

    pub fn serialize<S: Serializer>(said: &Option<u64>, to: S) -> Result<S::Ok, S::Error> {
        said.map(|thousandths| thousandths as f64 / 1000.0).serialize(to)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(from: D) -> Result<Option<u64>, D::Error> {
        match Option::<f64>::deserialize(from)? {
            None => Ok(None),
            Some(number) => match of(number) {
                Some(thousandths) => Ok(Some(thousandths)),
                None => Err(D::Error::custom(format!("{number} is not a number above zero"))),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_setting_is_said_as_its_flag_says_it_and_default_sets_it_back() {
        assert_eq!(thousandths_or_default("2.5"), Ok(To::Value(2500)));
        assert_eq!(thousandths_or_default("default"), Ok(To::Default));
        for wrong in ["0", "-1", "lots", "NaN", "inf", ""] {
            let error = thousandths_or_default(wrong).unwrap_err();
            assert!(error.contains("nor `default`"), "{wrong:?}: {error}");
        }
        assert_eq!(whole_or_default("0"), Ok(To::Value(0)), "0 is a share: never hold for disk");
        assert_eq!(whole_or_default("default"), Ok(To::Default));
        assert!(whole_or_default("5%").is_err() && whole_or_default("1.5").is_err());
        assert_eq!(port_or_default("7070"), Ok(To::Value(7070)));
        assert_eq!(port_or_default("default"), Ok(To::Default));
        assert!(port_or_default("70000").is_err());
    }

    #[test]
    fn what_a_run_names_is_laid_over_what_the_host_says_and_the_rest_is_kept() {
        let kept = Settings {
            hot_cores: Some(4000),
            admit_load: Some(400),
            panel: Some(7070),
            ..Settings::default()
        };
        assert_eq!(kept.clone().with(&Named::default()), kept, "naming nothing keeps it all");
        let named = Named {
            hot_cores: Some(To::Default),
            hot_memory: Some(To::Value(8000)),
            admit_load: Some(To::Value(400)),
            ..Named::default()
        };
        let now = kept.clone().with(&named);
        assert_eq!(
            now,
            Settings {
                hot_memory: Some(8000),
                admit_load: Some(400),
                panel: Some(7070),
                ..Settings::default()
            }
        );
        // each change, from what to what, in the units of its flag; a setting
        // named as it was is none
        let changes = Value::Object(kept.changes(&now));
        assert_eq!(
            changes,
            json!({
                "hot_cores": {"from": 4.0, "to": null},
                "hot_memory": {"from": null, "to": 8.0},
            })
        );
        assert!(now.changes(&now).is_empty());
    }

    #[test]
    fn the_file_keeps_each_setting_in_the_units_of_its_flag() {
        let settings = Settings {
            hot_cores: Some(2500),
            hot_written: Some(20_000),
            admit_disk: Some(0),
            ..Settings::default()
        };
        let said = serde_json::to_value(&settings).unwrap();
        assert_eq!(said["hot_cores"], json!(2.5));
        assert_eq!(said["hot_written"], json!(20.0));
        assert_eq!(said["admit_disk"], json!(0));
        assert_eq!(said["hot_memory"], Value::Null);
        assert_eq!(serde_json::from_value::<Settings>(said).unwrap(), settings);
        // what it leaves out is the default; a number no limit can be is refused
        assert_eq!(serde_json::from_str::<Settings>("{}").unwrap(), Settings::default());
        let wrong = serde_json::from_str::<Settings>(r#"{"hot_cores": -1}"#).unwrap_err();
        assert!(wrong.to_string().contains("not a number above zero"), "{wrong}");
        // what another afkconvoy says beside them is kept as it is: an older
        // one still reads the host sleep it was told to keep off
        let other = json!({"hot_cores": 2.5, "said_by_another": {"a": 1},
            "no_host_sleep": true, "host_sleep_hours": "22:00-06:00"});
        let read = serde_json::from_value::<Settings>(other).unwrap();
        assert_eq!(read.hot_cores, Some(2500));
        let written = serde_json::to_value(read.with(&Named::default())).unwrap();
        assert_eq!(written["said_by_another"], json!({"a": 1}));
        assert_eq!(written["no_host_sleep"], json!(true));
        assert_eq!(written["host_sleep_hours"], json!("22:00-06:00"));
        // the limits the sampler takes, in bytes
        let hot = Settings { hot_memory: Some(8000), ..settings }.hot();
        assert_eq!((hot.millicores, hot.resident), (Some(2500), Some(8 << 30)));
        assert_eq!(hot.written, Some(20 << 30));
    }

    #[test]
    fn a_run_keeps_the_host_s_name_once_and_a_name_said_holds_over_it() {
        let dir = tempfile::tempdir().unwrap();
        let list = List::at(dir.path().join("checkouts.json"));
        pin_host_name(&list).unwrap();
        assert_eq!(list.read().unwrap().settings.host_name, Some(system_host_name()));
        let named = Named {
            host_name: Some(host_name_or_default("old.local").unwrap()),
            ..Named::default()
        };
        let changed = Value::Object(change(&list, &named).unwrap());
        assert_eq!(changed["host_name"]["to"], "old.local");
        pin_host_name(&list).unwrap();
        assert_eq!(list.read().unwrap().settings.host_name.as_deref(), Some("old.local"));
        let readings = crate::admission::Readings::new();
        assert_eq!(json(&list, dir.path(), &readings)["in_force"]["host_name"], "old.local");
        assert!(host_name_or_default(" ").is_err());
        let reset = Named { host_name: Some(To::Default), ..Named::default() };
        change(&list, &reset).unwrap();
        pin_host_name(&list).unwrap();
        assert_eq!(list.read().unwrap().settings.host_name, Some(system_host_name()));
    }

    #[test]
    fn a_change_is_written_beside_the_checkouts_and_one_that_names_nothing_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let list = List::at(dir.path().join("checkouts.json"));
        assert!(change(&list, &Named::default()).unwrap().is_empty());
        assert!(!list.file().exists(), "nothing named, nothing written");
        let named = Named { admit_load: Some(To::Value(400)), ..Named::default() };
        let changed = change(&list, &named).unwrap();
        assert_eq!(Value::Object(changed), json!({"admit_load": {"from": null, "to": 400}}));
        assert_eq!(list.read().unwrap().settings.admit_load, Some(400));
        assert!(change(&list, &named).unwrap().is_empty(), "named as it is: no change");
        let readings = crate::admission::Readings::new();
        let answered = json(&list, dir.path(), &readings);
        assert_eq!(answered["said"]["admit_load"], 400);
        assert_eq!(answered["in_force"]["admit_load"], 400);
        assert_eq!(answered["in_force"]["admit_memory"], SHARES.memory);
        assert_eq!(answered["in_force"]["admit_inspect_after"], SPAN);
        assert!(answered["in_force"].get("no_host_sleep").is_none(), "{answered}");
        assert!(answered["in_force"]["hot_cores"].as_f64().unwrap() >= 1.0);
    }

    #[test]
    fn an_old_train_lanes_key_loads_and_the_next_write_drops_it() {
        let dir = tempfile::tempdir().unwrap();
        let list = List::at(dir.path().join("checkouts.json"));
        assert_eq!(
            Settings::default().lanes(),
            afkconvoy_core::exclusive::Lanes { worker: GATE_LANES, fast: FAST_LANES, slow: 1 }
        );
        let kept = r#"{"settings": {"train_lanes": 0, "gate_lanes": 2, "fast_lanes": 3, "said_by_another": 1}}"#;
        std::fs::write(list.file(), kept).unwrap();
        let read = list.read().unwrap();
        assert_eq!(read.settings.gate_lanes, Some(2));
        assert_eq!(read.settings.train_lanes, Some(0), "an old file still loads");
        let readings = crate::admission::Readings::new();
        let answered = json(&list, dir.path(), &readings);
        assert!(answered["said"].get("train_lanes").is_none(), "{answered}");
        assert!(answered["in_force"].get("train_lanes").is_none(), "{answered}");
        assert!(answered["in_force"].get("ignored").is_none(), "{answered}");
        assert_eq!(answered["in_force"]["gate_lanes"], 2);
        assert_eq!(answered["in_force"]["fast_lanes"], 3);
        let named = Named { admit_load: Some(To::Value(400)), ..Named::default() };
        change(&list, &named).unwrap();
        let text: Value =
            serde_json::from_str(&std::fs::read_to_string(list.file()).unwrap()).unwrap();
        assert!(text["settings"].get("train_lanes").is_none(), "{text}");
        assert_eq!(text["settings"]["gate_lanes"], 2);
        assert_eq!(text["settings"]["admit_load"], 400);
        assert_eq!(text["settings"]["said_by_another"], 1);
    }
}
