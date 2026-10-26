//! Host settings: what a host says for every fleet instance on it — whether
//! it may be put to sleep and in which hours, the limits past which a worker is hot, the shares
//! admission holds back at, the panel's port (docs/adr/0023). They are kept
//! in the checkout list's file, beside the checkouts, and changed by any
//! `run` that names one: that `run` writes them, and every fleet instance
//! reads them where it uses them — admission at each tick, the hot limits at
//! each sample, host sleep at each check — so a change holds for every one at
//! once, and none is started again for it. The panel's port alone is read
//! once, by the host runner as it starts. A `run` names one by its flag, or
//! by the variable of its environment that stands for that flag; nothing
//! else reads those variables.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use afkconvoy_core::admission::SHARES;
use afkconvoy_core::host_inspection::SPAN;
use afkconvoy_core::host_sleep::{Hours, SLEEP_HOURS};
use anyhow::{Result, bail};
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
    /// `--no-host-sleep`: no fleet on the host puts it to sleep.
    pub no_host_sleep: bool,
    /// `--host-sleep-hours`: the hours of its local time a fleet may put the
    /// host to sleep in, as said: `22:00-06:00`.
    #[serde(with = "hours")]
    pub host_sleep_hours: Option<Hours>,
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
    /// reason before the host is inspected.
    pub admit_inspect_after: Option<u64>,
    /// `--exclusive-seconds`: seconds a running command may hold others up.
    pub exclusive_seconds: Option<u64>,
    /// `--gate-lanes`: worker commands that may run across different repos.
    pub gate_lanes: Option<u64>,
    /// `--train-lanes`: train gates that may run across different repos.
    pub train_lanes: Option<u64>,
    /// `--panel`: the port the host runner serves the panel on; `PANEL_PORT`
    /// where the host says none.
    pub panel: Option<u16>,
    /// `--watch-dir`: the operator's directory; none starts no watch.
    pub watch_dir: Option<PathBuf>,
    /// `--watch-every`: seconds between the host runner's looks.
    pub watch_every: Option<u64>,
    /// What another afkconvoy says beside them, kept as it is.
    #[serde(flatten)]
    rest: Map<String, Value>,
}

/// The port the host runner serves the panel on where the host settings say
/// none.
pub const PANEL_PORT: u16 = 7070;
/// How often the host runner looks at its host where it says no interval.
pub const WATCH_EVERY: u64 = 300;

/// What a `run` named of the host settings; each it left unnamed is `None`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Named {
    /// `--no-host-sleep` (true) or `--host-sleep` (false).
    pub no_host_sleep: Option<bool>,
    pub host_sleep_hours: Option<To<Hours>>,
    pub hot_cores: Option<To<u64>>,
    pub hot_memory: Option<To<u64>>,
    pub hot_written: Option<To<u64>>,
    pub admit_load: Option<To<u64>>,
    pub admit_memory: Option<To<u64>>,
    pub admit_disk: Option<To<u64>>,
    pub admit_inspect_after: Option<To<u64>>,
    pub exclusive_seconds: Option<To<u64>>,
    pub gate_lanes: Option<To<u64>>,
    pub train_lanes: Option<To<u64>>,
    pub panel: Option<To<u16>>,
    pub watch_dir: Option<To<PathBuf>>,
    pub watch_every: Option<To<u64>>,
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

/// `--host-sleep-hours` as said: `HH:MM-HH:MM`, or `default`.
pub fn hours_or_default(said: &str) -> Result<To<Hours>, String> {
    To::parse(said, |said| Hours::parse(said).map_err(|why| format!("{why}; or `default`")))
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
/// its whole path, or `default` for none.
pub fn watch_dir_or_default(said: &str) -> Result<To<PathBuf>, String> {
    To::parse(said, |said| {
        std::fs::canonicalize(said)
            .ok()
            .filter(|dir| dir.is_dir())
            .ok_or_else(|| format!("`{said}` is no directory, nor `default`"))
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
        if let Some(no_host_sleep) = named.no_host_sleep {
            self.no_host_sleep = no_host_sleep;
        }
        lay(&mut self.host_sleep_hours, named.host_sleep_hours.as_ref());
        lay(&mut self.hot_cores, named.hot_cores.as_ref());
        lay(&mut self.hot_memory, named.hot_memory.as_ref());
        lay(&mut self.hot_written, named.hot_written.as_ref());
        lay(&mut self.admit_load, named.admit_load.as_ref());
        lay(&mut self.admit_memory, named.admit_memory.as_ref());
        lay(&mut self.admit_disk, named.admit_disk.as_ref());
        lay(&mut self.admit_inspect_after, named.admit_inspect_after.as_ref());
        lay(&mut self.exclusive_seconds, named.exclusive_seconds.as_ref());
        lay(&mut self.gate_lanes, named.gate_lanes.as_ref());
        lay(&mut self.train_lanes, named.train_lanes.as_ref());
        lay(&mut self.panel, named.panel.as_ref());
        lay(&mut self.watch_dir, named.watch_dir.as_ref());
        lay(&mut self.watch_every, named.watch_every.as_ref());
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
        self.gate_lanes.unwrap_or(1).max(1)
    }
    pub fn train_lanes(&self) -> u64 {
        self.train_lanes.unwrap_or(1).max(1)
    }

    /// The sleep hours it says, else the default.
    pub fn sleep_hours(&self) -> Hours {
        self.host_sleep_hours.unwrap_or(SLEEP_HOURS)
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
    /// worked out for this host.
    pub fn in_force(&self, dir: &Path) -> Map<String, Value> {
        let limits = hot::limits(self.hot(), dir);
        let gib = |bytes: u64| (bytes as u128 * 1000 / (1 << 30)) as f64 / 1000.0;
        let in_force = json!({
            "no_host_sleep": self.no_host_sleep,
            "host_sleep_hours": self.sleep_hours().to_string(),
            "hot_cores": limits.millicores as f64 / 1000.0,
            "hot_memory": gib(limits.resident),
            "hot_written": gib(limits.written),
            "admit_load": self.admit_load.unwrap_or(SHARES.load),
            "admit_memory": self.admit_memory.unwrap_or(SHARES.memory),
            "admit_disk": self.admit_disk.unwrap_or(SHARES.disk),
            "admit_inspect_after": self.admit_inspect_after.unwrap_or(SPAN),
            "exclusive_seconds": self.exclusive_seconds(),
            "gate_lanes": self.gate_lanes(),
            "train_lanes": self.train_lanes(),
            "panel": self.panel_port(),
            "watch_dir": self.watch_dir,
            "watch_every": self.watch_every(),
        });
        match in_force {
            Value::Object(in_force) => in_force,
            _ => unreachable!("built as an object"),
        }
    }
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

/// What this host's settings say of host sleep now: the sleep hours a fleet
/// may put it to sleep in; `None` where they keep it awake, or cannot be read.
pub fn host_sleep() -> Option<Hours> {
    let settings = of_host().ok().filter(|settings| !settings.no_host_sleep)?;
    Some(settings.sleep_hours())
}

/// Whether no fleet may put this host to sleep now: its settings say so, or
/// cannot be read.
pub fn kept_awake() -> bool {
    host_sleep().is_none()
}

/// Has the host say what `named` names, in `list` → what changed, as
/// `Settings::changes` says it. One that names nothing changes nothing and
/// writes nothing; one that names a setting as it is changes nothing either.
/// Host sleep is not said on a host it cannot be had on.
pub fn change(list: &List, named: &Named) -> Result<Map<String, Value>> {
    if named.is_empty() {
        return Ok(Map::new());
    }
    if named.no_host_sleep == Some(false) && !cfg!(target_os = "macos") {
        bail!(
            "this host is not macOS, the one a fleet can put to sleep once every fleet on it is \
             idle: `--host-sleep` cannot be said here, and it stays awake"
        );
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
pub fn json(list: &List, dir: &Path) -> Value {
    match list.read() {
        Ok(listed) => json!({
            "file": list.file(),
            "said": listed.settings.said(),
            "in_force": listed.settings.in_force(dir),
        }),
        Err(error) => json!({"file": list.file(), "error": format!("{error:#}")}),
    }
}

/// The variables of a `run`'s environment that name a host setting as its
/// flag does, where the flag is not given.
pub const NO_HOST_SLEEP_ENV: &str = "AFKCONVOY_NO_HOST_SLEEP";
pub const HOST_SLEEP_HOURS_ENV: &str = "AFKCONVOY_HOST_SLEEP_HOURS";
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

/// Sleep hours as the file keeps them: as they are said, `22:00-06:00`.
mod hours {
    use afkconvoy_core::host_sleep::Hours;
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(said: &Option<Hours>, to: S) -> Result<S::Ok, S::Error> {
        match said {
            Some(hours) => to.collect_str(hours),
            None => to.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(from: D) -> Result<Option<Hours>, D::Error> {
        match Option::<String>::deserialize(from)? {
            None => Ok(None),
            Some(said) => Hours::parse(&said).map(Some).map_err(D::Error::custom),
        }
    }
}

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
        let night = Hours::parse("22:00-06:00").unwrap();
        assert_eq!(hours_or_default("22:00-06:00"), Ok(To::Value(night)));
        assert_eq!(hours_or_default("default"), Ok(To::Default));
        for wrong in ["06:00-06:00", "6pm-6am", ""] {
            let error = hours_or_default(wrong).unwrap_err();
            assert!(error.contains("HH:MM-HH:MM") && error.contains("`default`"), "{error}");
        }
        assert_eq!(port_or_default("7070"), Ok(To::Value(7070)));
        assert_eq!(port_or_default("default"), Ok(To::Default));
        assert!(port_or_default("70000").is_err());
    }

    #[test]
    fn what_a_run_names_is_laid_over_what_the_host_says_and_the_rest_is_kept() {
        let kept = Settings {
            no_host_sleep: true,
            hot_cores: Some(4000),
            admit_load: Some(400),
            panel: Some(7070),
            ..Settings::default()
        };
        assert_eq!(kept.clone().with(&Named::default()), kept, "naming nothing keeps it all");
        let named = Named {
            no_host_sleep: Some(false),
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
                "no_host_sleep": {"from": true, "to": false},
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
        assert_eq!(said["host_sleep_hours"], Value::Null);
        assert_eq!(serde_json::from_value::<Settings>(said).unwrap(), settings);
        // sleep hours as they are said; hours that cover nothing are refused
        let night = Settings { host_sleep_hours: Hours::parse("22:00-06:00").ok(), ..settings };
        let said = serde_json::to_value(&night).unwrap();
        assert_eq!(said["host_sleep_hours"], json!("22:00-06:00"));
        assert_eq!(serde_json::from_value::<Settings>(said).unwrap(), night);
        let wrong = r#"{"host_sleep_hours": "06:00-06:00"}"#;
        let wrong = serde_json::from_str::<Settings>(wrong).unwrap_err();
        assert!(wrong.to_string().contains("covers no time of the day"), "{wrong}");
        let settings = Settings { host_sleep_hours: None, ..night };
        // what it leaves out is the default; a number no limit can be is refused
        assert_eq!(serde_json::from_str::<Settings>("{}").unwrap(), Settings::default());
        let wrong = serde_json::from_str::<Settings>(r#"{"hot_cores": -1}"#).unwrap_err();
        assert!(wrong.to_string().contains("not a number above zero"), "{wrong}");
        // what another afkconvoy says beside them is kept as it is
        let other = json!({"hot_cores": 2.5, "said_by_another": {"a": 1}});
        let read = serde_json::from_value::<Settings>(other).unwrap();
        assert_eq!(read.hot_cores, Some(2500));
        let written = serde_json::to_value(read.with(&Named::default())).unwrap();
        assert_eq!(written["said_by_another"], json!({"a": 1}));
        // the limits the sampler takes, in bytes
        let hot = Settings { hot_memory: Some(8000), ..settings }.hot();
        assert_eq!((hot.millicores, hot.resident), (Some(2500), Some(8 << 30)));
        assert_eq!(hot.written, Some(20 << 30));
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
        let answered = json(&list, dir.path());
        assert_eq!(answered["said"]["admit_load"], 400);
        assert_eq!(answered["in_force"]["admit_load"], 400);
        assert_eq!(answered["in_force"]["admit_memory"], SHARES.memory);
        assert_eq!(answered["in_force"]["admit_inspect_after"], SPAN);
        assert_eq!(answered["in_force"]["host_sleep_hours"], "00:00-06:00");
        // the sleep hours, changed from what to what, and back
        let night = Hours::parse("22:00-06:00").unwrap();
        let named = Named { host_sleep_hours: Some(To::Value(night)), ..Named::default() };
        let changed = Value::Object(change(&list, &named).unwrap());
        assert_eq!(changed, json!({"host_sleep_hours": {"from": null, "to": "22:00-06:00"}}));
        assert_eq!(json(&list, dir.path())["in_force"]["host_sleep_hours"], "22:00-06:00");
        let named = Named { host_sleep_hours: Some(To::Default), ..Named::default() };
        let changed = Value::Object(change(&list, &named).unwrap());
        assert_eq!(changed, json!({"host_sleep_hours": {"from": "22:00-06:00", "to": null}}));
        assert!(answered["in_force"]["hot_cores"].as_f64().unwrap() >= 1.0);
    }
}
