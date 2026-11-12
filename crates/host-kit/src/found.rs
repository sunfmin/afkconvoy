//! The harnesses a machine has: each known harness whose program the login
//! shell of the host's user finds.

use std::time::Duration;

use afkconvoy_harness::Harness;

use crate::shell::Shell;

/// How long a login shell is given to say whether a program is there.
pub const TIMEOUT: Duration = Duration::from_secs(20);

/// Whether `harness`'s program is found in `shell` → `Ok(())`; an error says
/// why it is taken as not installed: it was not found, the shell did not
/// answer in time, or it could not be started.
pub fn found(shell: &dyn Shell, harness: Harness) -> Result<(), String> {
    let question = format!("type -- '{}'", harness.program());
    match shell.run(&question, TIMEOUT) {
        Ok(Some((true, _))) => Ok(()),
        Ok(Some((false, _))) => Err(format!("`{question}` found nothing")),
        Ok(None) => Err(format!("`{question}` did not answer in time")),
        Err(error) => Err(format!("{error:#}: {question}")),
    }
}

/// Every known harness `shell` finds, in the order `Harness::ALL` names them,
/// each asked at once.
pub fn harnesses<S: Shell + Sync>(shell: &S) -> Vec<Harness> {
    std::thread::scope(|scope| {
        let asking: Vec<_> = Harness::ALL
            .iter()
            .map(|&harness| scope.spawn(move || found(shell, harness).ok().map(|()| harness)))
            .collect();
        asking.into_iter().filter_map(|asked| asked.join().ok().flatten()).collect()
    })
}
