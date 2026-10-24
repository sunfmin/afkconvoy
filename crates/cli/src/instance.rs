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
use crate::{hot, kit, probe};

/// How `run` asks for its checkout's fleet instance: what it named for that
/// instance alone, kept on the checkout list so that a host runner started
/// again starts it the same way; what it named of the host, which holds for
/// this start; and the knobs that exist for the tests.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Start {
    pub named: Named,
    /// `--panel`: served from this start; `panel --open` opens it later.
    pub panel: Option<u16>,
    pub no_host_sleep: bool,
    pub hot: hot::Said,
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
/// Everything before its first tick may refuse it: a host it cannot put to
/// sleep, the host kit, the startup probe, the panel's port. Its startup
/// probe's warnings are said as `Line::Warning`.
pub fn hold(checkout: &Repo, start: &Start, holder: Holder) -> Result<Value> {
    if !start.no_host_sleep && !cfg!(target_os = "macos") {
        bail!(
            "this host is not macOS, the one a fleet can put to sleep once every fleet on it is \
             idle: start it with `--no-host-sleep`, which keeps it awake"
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
        no_host_sleep: start.no_host_sleep,
        host_sleep_after: knobs.host_sleep_after,
        hot: hot::limits(start.hot, &checkout.dir),
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
/// printed it, until SIGTERM or SIGINT. The checkout list and any host runner
/// are left alone.
pub fn foreground(checkout: &Repo, start: &Start) -> Result<Value> {
    let stop = Stop::listen();
    let echo: Echo = Arc::new(|line| match line {
        Line::Kept(entry) => println!("{}", entry.said()),
        Line::Said(text) => println!("{text}"),
        Line::Warning(warning) => probe::announce(&[warning.to_string()]),
    });
    hold(checkout, start, Holder { stop: stop.instance(), echo })
}
