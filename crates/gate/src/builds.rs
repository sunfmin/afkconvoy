//! What every build the fleet starts runs with (docs/adr/0088): the worker's
//! terminal exports it, and the gate's command is started with it.

/// cargo's opt-out of its fine-grain unit locks, by name and value. With
/// them, the cargos of the worktrees of one repo, each building into one
/// shared build directory, hold some unit locks shared while they wait on
/// others exclusively, in no fixed order: they deadlock, each at no CPU,
/// waiting forever (sunfmin/control#1249). Without them, each build takes
/// its build directory's one lock whole, and waits only on a build that is
/// running: builds of one build directory run one after another.
pub const SERIAL_BUILDS: (&str, &str) = ("__CARGO_TEMPORARY_FINE_GRAIN_LOCKING_OPT_OUT", "1");
