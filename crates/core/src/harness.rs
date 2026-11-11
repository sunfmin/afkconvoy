//! A harness (docs/adr/0013): the program a worker is a session of. All that
//! is one harness's is the harness crate's (docs/adr/0058): nothing here
//! names a flag, a variable, a hook event or a path of one, nor where it
//! keeps its session, nor which of its lines start or end a turn
//! (docs/adr/0044). A worker's start in its role is put to it as a neutral
//! start (`role::Start::in_role`).

pub use afkconvoy_harness::*;
