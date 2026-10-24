//! Serving the panel: one page, held in memory, on a port of this host only.
//! The host runner shows it a new page after each tick of any fleet instance
//! it holds, and every browser that has the page open is told so and fetches
//! it. Nothing a browser sends is read past its request line, and nothing
//! here reaches the fleet. The page is served from when it is opened, at one
//! address, until it is closed.

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::panel::LIVE_SCRIPT;

/// How long an open page hears nothing before it is told the host runner is
/// still there.
const QUIET: Duration = Duration::from_secs(15);

/// The page as it stands, and how many were shown before it.
struct Shown {
    page: Mutex<(u64, String)>,
    changed: Condvar,
    /// The port the page is served on, once it is.
    port: Mutex<Option<u16>>,
    /// Told once it is served no more: the thread that takes browsers on its
    /// port lets the port go.
    closed: AtomicBool,
    taking: Mutex<Option<JoinHandle<()>>>,
}

impl Shown {
    fn now(&self) -> (u64, String) {
        self.page.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

/// Where a panel served on `port` of this host is read.
pub fn address(port: u16) -> String {
    format!("http://127.0.0.1:{port}/")
}

/// The panel of the host runner, held for as long as it runs: its page, and
/// the one port it is served on once it was opened. A clone is the same
/// panel.
#[derive(Clone)]
pub struct Live {
    shown: Arc<Shown>,
}

impl Live {
    /// A panel that shows `page`, served nowhere yet.
    pub fn new(page: String) -> Live {
        let page = Mutex::new((0, page));
        let (changed, port) = (Condvar::new(), Mutex::new(None));
        let (closed, taking) = (AtomicBool::new(false), Mutex::new(None));
        Live { shown: Arc::new(Shown { page, changed, port, closed, taking }) }
    }

    /// Serves the panel on `port` of this host — 0: any free one — to this
    /// host only: the page names every open issue of every repo → the port
    /// it is served on. A panel already served stays where it is, whatever
    /// `port` asks, and one that could not be served is served nowhere: a
    /// later call may open it.
    pub fn open(&self, port: u16) -> Result<u16> {
        // Held until it is served: of two that ask at once, one opens it.
        let mut served = self.shown.port.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(port) = *served {
            return Ok(port);
        }
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
            .with_context(|| format!("the panel cannot be served on port {port}"))?;
        let port = listener.local_addr().context("the panel's port cannot be read")?.port();
        let serving = self.shown.clone();
        let taking = std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                if serving.closed.load(Ordering::SeqCst) {
                    break;
                }
                let shown = serving.clone();
                // One thread a browser: an open page holds its `events` for good.
                std::thread::spawn(move || answer(stream, &shown));
            }
        });
        *self.shown.taking.lock().unwrap_or_else(PoisonError::into_inner) = Some(taking);
        *served = Some(port);
        Ok(port)
    }

    /// Serves the panel no more → once its port is let go, for whoever
    /// opens one on it next: a host runner started again. A browser that has
    /// it open keeps what it shows, and is told nothing more.
    pub fn close(&self) {
        self.shown.closed.store(true, Ordering::SeqCst);
        let port = *self.shown.port.lock().unwrap_or_else(PoisonError::into_inner);
        let taking = self.shown.taking.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let (Some(port), Some(taking)) = (port, taking) {
            // Whoever takes browsers waits for one: this is it, and it sees it closed.
            let _ = TcpStream::connect((Ipv4Addr::LOCALHOST, port));
            let _ = taking.join();
        }
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
        let served = Live::new("<p>first</p>".to_string());
        let port = served.open(0).unwrap();
        let first = get(port, "/");
        assert!(first.starts_with("HTTP/1.1 200 OK\r\nContent-Type: text/html"), "{first}");
        assert!(first.ends_with("\r\n\r\n<p>first</p>"), "{first}");
        served.show("<p>second — 第二</p>".to_string());
        let second = get(port, "/?again");
        assert!(second.ends_with("<p>second — 第二</p>"), "{second}");
        assert!(second.contains("Content-Length: 24\r\n"), "bytes, not characters: {second}");
        assert!(get(port, "/live.js").contains("new EventSource(\"events\")"));
        assert!(get(port, "/../etc/passwd").starts_with("HTTP/1.1 404 "));
    }

    #[test]
    fn an_open_page_is_told_of_every_page_shown_after_it_opened() {
        let served = Live::new(String::new());
        let mut events = BufReader::new(asked(served.open(0).unwrap(), "/events"));
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
    fn a_port_that_is_taken_is_refused_by_name_and_the_panel_can_still_be_opened() {
        let taken = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = taken.local_addr().unwrap().port();
        let live = Live::new("<p>page</p>".to_string());
        let error = live.open(port).err().unwrap();
        assert!(format!("{error:#}").contains(&format!("port {port}")), "{error:#}");
        let free = live.open(0).unwrap();
        assert!(get(free, "/").ends_with("<p>page</p>"));
    }

    #[test]
    fn a_panel_opened_again_stays_at_its_one_address() {
        let live = Live::new("<p>before</p>".to_string());
        // what was shown before anyone could read it is the page it opens with
        live.show("<p>shown unserved</p>".to_string());
        let port = live.open(0).unwrap();
        assert_eq!(live.clone().open(0).unwrap(), port);
        let other = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap().local_addr().unwrap();
        assert_eq!(live.open(other.port()).unwrap(), port, "whatever port is asked");
        assert!(get(port, "/").ends_with("<p>shown unserved</p>"));
        assert!(TcpStream::connect(other).is_err(), "nothing is served on the port asked second");
    }

    #[test]
    fn a_panel_closed_lets_its_port_go_for_the_next_to_serve_on() {
        let live = Live::new("<p>before</p>".to_string());
        let port = live.open(0).unwrap();
        // a browser that has it open holds a connection on the port
        let _open = asked(port, "/events");
        live.close();
        let next = Live::new("<p>after</p>".to_string());
        assert_eq!(next.open(port).unwrap(), port, "the port was let go");
        assert!(get(port, "/").ends_with("<p>after</p>"));
        live.close();
        Live::new(String::new()).close();
    }
}
