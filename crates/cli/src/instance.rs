//! A fleet instance from its start to its end: what `run` asks for one, and
//! holding one — alone in this process (`run --foreground`) or in the host
//! runner beside the others (docs/adr/0023).

use std::sync::Arc;
use std::time::Duration;

use afkconvoy_core::claims::InstanceId;
use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::claims::Me;
use crate::git::Repo;
use crate::stop::Stop;
use crate::tick::{self, Echo, Holder, Line, Pacing};
use crate::{host_settings, kit, probe};

/// How `run` asks for its checkout's fleet instance: what it named for that
/// instance alone, kept on the checkout list so that a host runner started
/// again starts it the same way; the port its panel is served on from this
/// start; and the knobs that exist for the tests. What it named of the host
/// is the host settings' (`host_settings`), which the instance reads as it
/// goes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Start {
    pub named: Named,
    /// `--panel`: served from this start; `panel --open` opens it later.
    pub panel: Option<u16>,
    pub knobs: Knobs,
}

/// What a `run` named for its own checkout's fleet instance; what it left
/// unnamed is the default at each start.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Named {
    /// `--instance`, as it was given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub worker_no_proxy: Vec<String>,
}

impl Named {
    /// Whether it names nothing: every default holds.
    pub fn is_empty(&self) -> bool {
        *self == Named::default()
    }

    /// What it names, and of what it leaves unnamed, what `kept` names: a
    /// `run` laid over what the checkout list kept of the one before.
    pub fn or(self, kept: Named) -> Named {
        let worker_no_proxy = match self.worker_no_proxy.is_empty() {
            true => kept.worker_no_proxy,
            false => self.worker_no_proxy,
        };
        Named {
            instance: self.instance.or(kept.instance),
            host: self.host.or(kept.host),
            worker_no_proxy,
        }
    }
}

/// The knobs of the tests: never kept on the checkout list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Knobs {
    /// Stop after this many ticks [default: never].
    pub ticks: Option<u64>,
    /// Milliseconds one second of sleep takes.
    pub second_ms: u64,
    /// Seconds every fleet on this host must have been idle before it is put
    /// to sleep.
    pub host_sleep_after: u64,
    /// Seconds an agent is given to be ready for its prompt.
    pub ready_timeout: u64,
    /// The epoch seconds its clock reads, fixed: the global `--now`.
    pub now: Option<u64>,
}

/// Holds the fleet instance of `checkout` as `start` asks, until it is told
/// to stop or has made `start.knobs.ticks` → what its loop answers.
/// Everything before its first tick may refuse it: host settings that
/// cannot be read, or that have a host put to sleep that cannot be, the host
/// kit, the startup probe, the panel's port. Its startup probe's warnings are
/// said as `Line::Warning`.
pub fn hold(checkout: &Repo, start: &Start, holder: Holder) -> Result<Value> {
    if !host_settings::of_host()?.no_host_sleep && !cfg!(target_os = "macos") {
        bail!(
            "this host is not macOS, the one a fleet can put to sleep once every fleet on it is \
             idle: run `afkconvoy run --no-host-sleep`, which keeps it awake, a host setting"
        );
    }
    let (named, knobs) = (&start.named, &start.knobs);
    let fixed_now = knobs.now;
    let clock = move || fixed_now.unwrap_or_else(crate::clock);
    let now = clock();
    let host = named.host.clone().unwrap_or_else(crate::this_host);
    let instance = match &named.instance {
        Some(instance) => InstanceId::parse(instance).map_err(|why| anyhow!(why))?,
        None => {
            // Two checkouts started in one second by one process mint two ids.
            let checkout_key = afkconvoy_core::gate::key(&checkout.dir.to_string_lossy()) as u32;
            InstanceId::mint(&host, crate::clock() as u32 ^ std::process::id() ^ checkout_key)
        }
    };
    let mut me = Me { instance, host, now };
    let pacing = Pacing {
        ticks: knobs.ticks,
        second: Duration::from_millis(knobs.second_ms),
        host_sleep_after: knobs.host_sleep_after,
    };
    kit::ensure(now)?;
    let worker_no_proxy = &named.worker_no_proxy;
    let (agent, warnings) =
        crate::start_agent(checkout, worker_no_proxy, knobs.ready_timeout, now, false)?;
    for warning in &warnings {
        (holder.echo)(Line::Warning(warning));
    }
    tick::run(checkout, &mut me, &agent, &pacing, start.panel, clock, holder)
}

/// `run --foreground`: holds it in this process, printing every line as `run`
/// printed it, until SIGTERM or SIGINT. The checkout list's file and any host
/// runner are left alone: what it names of the host settings, `settings`,
/// holds in this process alone, laid over the host's.
pub fn foreground(checkout: &Repo, start: &Start, settings: host_settings::Named) -> Result<Value> {
    host_settings::lay_for_this_process(settings);
    let stop = Stop::listen();
    let echo: Echo = Arc::new(|line| match line {
        Line::Kept(entry) => println!("{}", entry.said()),
        Line::Said(text) => println!("{text}"),
        Line::Warning(warning) => probe::announce(&[warning.to_string()]),
    });
    hold(checkout, start, Holder { stop: stop.instance(), echo })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_a_run_names_is_laid_over_what_was_kept_field_by_field() {
        let kept = Named {
            instance: Some("me-h1-1a2b3c4d".into()),
            host: Some("h1".into()),
            worker_no_proxy: vec!["api.example.com".into()],
        };
        assert_eq!(Named::default().or(kept.clone()), kept, "naming nothing keeps it all");
        let named = Named { host: Some("h2".into()), ..Named::default() };
        let laid = named.or(kept.clone());
        assert_eq!(laid.host.as_deref(), Some("h2"));
        assert_eq!(
            (&laid.instance, &laid.worker_no_proxy),
            (&kept.instance, &kept.worker_no_proxy)
        );
        let bypass = Named { worker_no_proxy: vec!["pi=x".into()], ..Named::default() };
        assert_eq!(bypass.clone().or(kept).worker_no_proxy, bypass.worker_no_proxy);
    }
}
