//! Stands in for the host runner of another afkconvoy in the tests:
//! `fake_host_runner <version>`. It holds the host as a host runner does —
//! the lock in the cache directory, saying its pid and that version — and
//! answers on the host runner's socket as one of an afkconvoy that cannot read
//! the question: with an error, and with itself as `host_runner`. Told to
//! stop (SIGTERM, SIGINT), it removes its socket, lets the host go and ends.

use std::io::{BufRead, BufReader, Write};
use std::time::Duration;

use afkconvoy::copies::cache;
use afkconvoy::runner_socket::{self, SOCKET};
use afkconvoy::stop::Stop;
use serde_json::json;

fn main() {
    let version = std::env::args().nth(1).expect("fake_host_runner <version>");
    let stop = Stop::listen();
    let dir = cache().unwrap();
    std::fs::create_dir_all(&dir).unwrap();
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("host-runner.lock"))
        .unwrap();
    lock.lock().unwrap();
    let pid = std::process::id();
    let holder = json!({"pid": pid, "version": version, "started": 0});
    lock.set_len(0).unwrap();
    (&lock).write_all(holder.to_string().as_bytes()).unwrap();

    let listener = runner_socket::listen().unwrap();
    let about = json!({"pid": pid, "version": version, "log": dir.join("host-runner.log")});
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut asked = String::new();
            let _ = BufReader::new(&stream).read_line(&mut asked);
            let answer = json!({
                "error": format!(
                    "this host runner (afkconvoy {version}) cannot read {}: it is another version \
                     than the afkconvoy that asked",
                    asked.trim()
                ),
                "host_runner": about,
            });
            let _ = writeln!(&stream, "{answer}");
        }
    });
    while !stop.told() {
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = std::fs::remove_file(dir.join(SOCKET));
    drop(lock);
}
