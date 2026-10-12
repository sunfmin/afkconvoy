//! Serving the panel: one page, held in memory, on a port of this host only.
//! `run` shows it a new page after each tick, and every browser that has the
//! page open is told so and fetches it. Nothing a browser sends is read past
//! its request line, and nothing here reaches the fleet.

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use anyhow::{Context, Result};

use crate::panel::LIVE_SCRIPT;

/// How long an open page hears nothing before it is told the fleet instance
/// is still there.
const QUIET: Duration = Duration::from_secs(15);

/// The page as it stands, and how many were shown before it.
struct Shown {
    page: Mutex<(u64, String)>,
    changed: Condvar,
}

impl Shown {
    fn now(&self) -> (u64, String) {
        self.page.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

/// The serving end, held by `run` for as long as it runs.
pub struct Served {
    shown: Arc<Shown>,
    pub port: u16,
}

impl Served {
    /// Serves `page` on `port` of this host — 0: any free one — to this host
    /// only: the page names every open issue of the repo.
    pub fn open(port: u16, page: String) -> Result<Served> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
            .with_context(|| format!("the panel cannot be served on port {port}"))?;
        let port = listener.local_addr().context("the panel's port cannot be read")?.port();
        let shown = Arc::new(Shown { page: Mutex::new((0, page)), changed: Condvar::new() });
        let serving = shown.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let shown = serving.clone();
                // One thread a browser: an open page holds its `events` for good.
                std::thread::spawn(move || answer(stream, &shown));
            }
        });
        Ok(Served { shown, port })
    }

    /// Shows `page` from here on, and tells every open page there is one.
    pub fn show(&self, page: String) {
        let mut shown = self.shown.page.lock().unwrap_or_else(PoisonError::into_inner);
        *shown = (shown.0 + 1, page);
        self.shown.changed.notify_all();
    }
}

/// Answers one browser. Whatever goes wrong with it is its own loss.
fn answer(mut stream: TcpStream, shown: &Shown) {
    let _ = stream.set_read_timeout(Some(QUIET));
    let mut line = String::new();
    let read =
        stream.try_clone().map(BufReader::new).and_then(|mut from| from.read_line(&mut line));
    if read.is_err() {
        return;
    }
    let path = line.split_whitespace().nth(1).unwrap_or("").split('?').next().unwrap_or("");
    let _ = match path {
        "/" => whole(&mut stream, "200 OK", "text/html; charset=utf-8", &shown.now().1),
        "/live.js" => whole(&mut stream, "200 OK", "text/javascript; charset=utf-8", LIVE_SCRIPT),
        "/events" => events(&mut stream, shown),
        _ => whole(&mut stream, "404 Not Found", "text/plain; charset=utf-8", "no such page\n"),
    };
}

fn whole(stream: &mut TcpStream, status: &str, kind: &str, body: &str) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// One `tick` event for each page shown after the one the browser has, until
/// the browser is gone.
fn events(stream: &mut TcpStream, shown: &Shown) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\n\
         Connection: close\r\n\r\n"
    )?;
    let mut seen = shown.now().0;
    loop {
        let page = shown.page.lock().unwrap_or_else(PoisonError::into_inner);
        let waited = shown.changed.wait_timeout_while(page, QUIET, |page| page.0 == seen);
        let at = waited.unwrap_or_else(PoisonError::into_inner).0.0;
        if at == seen {
            // A comment: it keeps the connection, and finds a browser that left.
            write!(stream, ": still here\n\n")?;
        } else {
            seen = at;
            write!(stream, "event: tick\ndata: {at}\n\n")?;
        }
        stream.flush()?;
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;

    fn asked(port: u16, path: &str) -> TcpStream {
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        write!(stream, "GET {path} HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
        stream
    }

    fn get(port: u16, path: &str) -> String {
        let mut answer = String::new();
        asked(port, path).read_to_string(&mut answer).unwrap();
        answer
    }

    #[test]
    fn the_page_shown_last_is_the_page_served_and_nothing_else_is() {
        let served = Served::open(0, "<p>first</p>".to_string()).unwrap();
        let first = get(served.port, "/");
        assert!(first.starts_with("HTTP/1.1 200 OK\r\nContent-Type: text/html"), "{first}");
        assert!(first.ends_with("\r\n\r\n<p>first</p>"), "{first}");
        served.show("<p>second — 第二</p>".to_string());
        let second = get(served.port, "/?again");
        assert!(second.ends_with("<p>second — 第二</p>"), "{second}");
        assert!(second.contains("Content-Length: 24\r\n"), "bytes, not characters: {second}");
        assert!(get(served.port, "/live.js").contains("new EventSource(\"events\")"));
        assert!(get(served.port, "/../etc/passwd").starts_with("HTTP/1.1 404 "));
    }

    #[test]
    fn an_open_page_is_told_of_every_page_shown_after_it_opened() {
        let served = Served::open(0, String::new()).unwrap();
        let mut events = BufReader::new(asked(served.port, "/events"));
        let mut line = String::new();
        while line != "\r\n" {
            line.clear();
            events.read_line(&mut line).unwrap();
        }
        served.show("one".to_string());
        served.show("two".to_string());
        let mut told = String::new();
        while !told.contains("data: 2\n") {
            assert_ne!(events.read_line(&mut told).unwrap(), 0, "the events ended: {told}");
        }
        assert!(told.contains("event: tick\n"), "{told}");
    }

    #[test]
    fn a_port_that_is_taken_is_refused_by_name() {
        let served = Served::open(0, String::new()).unwrap();
        let error = Served::open(served.port, String::new()).err().unwrap();
        assert!(format!("{error:#}").contains(&format!("port {}", served.port)), "{error:#}");
    }
}
