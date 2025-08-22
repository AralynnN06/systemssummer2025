use clap::{Arg, ArgAction, Command};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_with::{serde_as, DurationMilliSeconds};
use std::collections::HashMap;
use std::io::{self, BufRead};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[serde_as]
#[derive(Debug, Clone, Serialize)]
pub struct WebsiteStatus {
    pub url: String,
    pub status: Result<u16, String>,
    #[serde_as(as = "DurationMilliSeconds<u64>")]
    pub response_time: Duration,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone)]
struct Config {
    worker_threads: usize,
    timeout: Duration,
    max_retries: usize,
    period: Option<Duration>, // None => run once; Some(d) => repeat every d
    headers: Vec<(String, String)>, // Header validations: (Name, ExpectedValue)
    contains: Option<String>,       // Body must contain this substring if set
    urls: Vec<String>,
}

#[derive(Debug, Default, Clone)]
struct UrlStats {
    checks: u64,
    successes: u64,
    total_response_ms: u128,
}
impl UrlStats {
    fn record(&mut self, ok: bool, rt: Duration) {
        self.checks += 1;
        if ok {
            self.successes += 1;
        }
        self.total_response_ms += rt.as_millis();
    }
    fn uptime(&self) -> f64 {
        if self.checks == 0 { 0.0 } else { (self.successes as f64) * 100.0 / (self.checks as f64) }
    }
    fn avg_ms(&self) -> f64 {
        if self.checks == 0 { 0.0 } else { (self.total_response_ms as f64) / (self.checks as f64) }
    }
}

fn parse_header(s: &str) -> Option<(String, String)> {
    if let Some((name, value)) = s.split_once(':') {
        Some((name.trim().to_string(), value.trim().to_string()))
    } else {
        None
    }
}

fn build_cli() -> Command {
    Command::new("sitecheck")
        .about("Concurrent Website Status Checker (threaded + channels)")
        .arg(
            Arg::new("threads")
                .short('n')
                .long("threads")
                .value_name("NUM")
                .help("Number of worker threads (default: 50)")
                .num_args(1),
        )
        .arg(
            Arg::new("timeout")
                .short('t')
                .long("timeout")
                .value_name("SECS")
                .help("Request timeout seconds (default: 5)")
                .num_args(1),
        )
        .arg(
            Arg::new("retries")
                .short('r')
                .long("retries")
                .value_name("NUM")
                .help("Max retries per website (default: 1)")
                .num_args(1),
        )
        .arg(
            Arg::new("period")
                .short('p')
                .long("period")
                .value_name("SECS")
                .help("If set, run periodically every SECS (default: run once)")
                .num_args(1),
        )
        .arg(
            Arg::new("file")
                .short('f')
                .long("file")
                .value_name("PATH")
                .help("File with one URL per line")
                .num_args(1),
        )
        .arg(
            Arg::new("header")
                .short('H')
                .long("header")
                .value_name("'Name: Value'")
                .help("Require response header to match value (repeatable)")
                .action(ArgAction::Append)
                .num_args(1),
        )
        .arg(
            Arg::new("contains")
                .long("contains")
                .value_name("TEXT")
                .help("Require response body to contain TEXT")
                .num_args(1),
        )
        .arg(
            Arg::new("urls")
                .help("List of URLs to check (http/https)")
                .num_args(0..)
                .value_name("URL"),
        )
        .after_help(
"EXAMPLES:
  sitecheck https://example.com https://rust-lang.org
  sitecheck -f urls.txt -n 80 -t 3 -r 2
  sitecheck -p 60 -H 'Server: nginx' --contains 'Welcome' https://example.com"
        )
}

fn read_urls_from_file(path: &PathBuf) -> io::Result<Vec<String>> {
    let f = std::fs::File::open(path)?;
    let reader = io::BufReader::new(f);
    Ok(reader
        .lines()
        .filter_map(Result::ok)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && !s.starts_with('#'))
        .collect())
}

fn build_agent(timeout: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(timeout)
        .timeout_read(timeout)
        .timeout_write(timeout)
        .redirects(2)
        .build()
}

/// Fetch once with validations. Returns (HTTP status, elapsed).
fn fetch_once(
    agent: &ureq::Agent,
    url: &str,
    headers_expected: &[(String, String)],
    contains: &Option<String>,
) -> Result<(u16, Duration), String> {
    let start = Instant::now();
    let resp = agent
        .get(url)
        .call()
        .map_err(|e| format!("request error: {e}"))?;

    let status = resp.status();

    // Header validation (case-insensitive name, exact value match)
    for (name, value) in headers_expected {
        // ureq uses case-insensitive header lookup
        let got = resp.header(name);
        match got {
            Some(v) if v == value => {}
            Some(v) => {
                return Err(format!("header mismatch: {} expected '{}' got '{}'", name, value, v));
            }
            None => {
                return Err(format!("missing required header: {}", name));
            }
        }
    }

    // Body validation (if requested)
    if let Some(needle) = contains {
        // Read body as string (NOTE: may be large; in production limit size or stream)
        let body = resp
            .into_string()
            .map_err(|e| format!("body read error: {e}"))?;
        if !body.contains(needle) {
            return Err(format!("body validation failed: missing substring '{}'", needle));
        }
        let elapsed = start.elapsed();
        Ok((status, elapsed))
    } else {
        // If we didn't read the body above, ensure we close it
        let _ = resp.into_reader(); // drop the reader; not strictly necessary
        let elapsed = start.elapsed();
        Ok((status, elapsed))
    }
}

/// Check a URL with retries & validations, returning a WebsiteStatus.
fn check_with_retries(
    agent: &ureq::Agent,
    url: &str,
    headers_expected: &[(String, String)],
    contains: &Option<String>,
    max_retries: usize,
) -> WebsiteStatus {
    let mut last_err: Option<String> = None;
    for attempt in 0..=max_retries {
        match fetch_once(agent, url, headers_expected, contains) {
            Ok((code, rt)) => {
                return WebsiteStatus {
                    url: url.to_string(),
                    status: Ok(code),
                    response_time: rt,
                    timestamp: Utc::now(),
                };
            }
            Err(e) => {
                last_err = Some(e);
                if attempt < max_retries {
                    // simple linear backoff
                    thread::sleep(Duration::from_millis(200 * (attempt as u64 + 1)));
                }
            }
        }
    }
    WebsiteStatus {
        url: url.to_string(),
        status: Err(last_err.unwrap_or_else(|| "unknown error".to_string())),
        response_time: Duration::from_millis(0),
        timestamp: Utc::now(),
    }
}

fn print_status_json(s: &WebsiteStatus) {
    // Pretty JSON line for each status
    match serde_json::to_string(s) {
        Ok(js) => println!("{}", js),
        Err(_) => println!("{:?}", s),
    }
}

fn summarize(stats: &HashMap<String, UrlStats>) {
    println!("--- stats summary ---");
    for (url, st) in stats {
        println!(
            "{} -> checks: {}, uptime: {:.1}%, avg_rt_ms: {:.1}",
            url,
            st.checks,
            st.uptime(),
            st.avg_ms()
        );
    }
    println!("---------------------");
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let m = build_cli().get_matches();

    let worker_threads: usize = m
        .get_one::<String>("threads")
        .and_then(|s| s.parse().ok())
        .unwrap_or(50);

    let timeout = Duration::from_secs(
        m.get_one::<String>("timeout")
            .and_then(|s| s.parse().ok())
            .unwrap_or(5),
    );

    let max_retries: usize = m
        .get_one::<String>("retries")
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);

    let period = m
        .get_one::<String>("period")
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_secs);

    let mut urls: Vec<String> = vec![];

    if let Some(path) = m.get_one::<String>("file") {
        let path = PathBuf::from(path);
        urls.extend(read_urls_from_file(&path)?);
    }

    if let Some(args) = m.get_many::<String>("urls") {
        urls.extend(args.into_iter().map(|s| s.to_string()));
    }

    if urls.is_empty() {
        eprintln!("No URLs provided. Provide positional URLs or -f <file>.");
        std::process::exit(1);
    }

    let headers: Vec<(String, String)> = m
        .get_many::<String>("header")
        .map(|vals| {
            vals.filter_map(|s| parse_header(s))
                .collect::<Vec<(String, String)>>()
        })
        .unwrap_or_default();

    let contains = m.get_one::<String>("contains").cloned();

    let cfg = Config {
        worker_threads,
        timeout,
        max_retries,
        period,
        headers,
        contains,
        urls,
    };

    // Graceful shutdown flag
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        ctrlc::set_handler(move || {
            eprintln!("
Ctrl+C detected, shutting down...");
            stop.store(true, Ordering::SeqCst);
        })?;
    }

    // Channels
    let (job_tx, job_rx_raw) = mpsc::channel::<String>();
    let job_rx = Arc::new(Mutex::new(job_rx_raw)); // share one receiver across workers
    let (res_tx, res_rx) = mpsc::channel::<WebsiteStatus>();

    // Spawn workers
    let mut workers = Vec::with_capacity(cfg.worker_threads);
    for _ in 0..cfg.worker_threads {
        let job_rx = Arc::clone(&job_rx);
        let res_tx = res_tx.clone();
        let headers = cfg.headers.clone();
        let contains = cfg.contains.clone();
        let timeout = cfg.timeout;
        let max_retries = cfg.max_retries;

        workers.push(thread::spawn(move || {
            let agent = build_agent(timeout);
            loop {
                // Lock only to receive the next job, then release before doing work
                let msg = {
                    let rx = job_rx.lock().unwrap();
                    rx.recv()
                };
                match msg {
                    Ok(url) => {
                        let status = check_with_retries(&agent, &url, &headers, &contains, max_retries);
                        let _ = res_tx.send(status);
                    }
                    Err(_) => break, // sender dropped => shutdown
                }
            }
        }));
    }
    drop(res_tx); // when all worker clones drop, results channel will close


    let mut stats: HashMap<String, UrlStats> = HashMap::new();
    let mut round: u64 = 0;

    // Main loop (one-shot or periodic)
    loop {
        round += 1;
        if stop.load(Ordering::SeqCst) {
            break;
        }

        // Enqueue this round's URLs
        for url in &cfg.urls {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            job_tx.send(url.clone()).ok();
        }

        // Collect this round's results
        let expected = cfg.urls.len();
        for _ in 0..expected {
            match res_rx.recv() {
                Ok(status) => {
                    let ok = status.status.is_ok();
                    print_status_json(&status);
                    stats
                        .entry(status.url.clone())
                        .or_default()
                        .record(ok, status.response_time);
                }
                Err(_) => break, // channel closed
            }
        }

        summarize(&stats);

        // If not periodic, we're done
        if cfg.period.is_none() {
            break;
        }

        // Sleep until the next round (or until interrupted)
        let period = cfg.period.unwrap();
        let mut slept = Duration::from_secs(0);
        while slept < period {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            let step = Duration::from_millis(200);
            thread::sleep(step);
            slept += step;
        }
    }

    // Shutdown: drop sender so workers exit, then join
    drop(job_tx);
    for w in workers {
        let _ = w.join();
    }

    eprintln!("Shutdown complete.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;

    #[test]
    fn test_success_ok() {
        let server = MockServer::start();

        let _m = server.mock(|when, then| {
            when.method(GET).path("/ok");
            then.status(200)
                .header("Server", "unit-test")
                .body("hello world");
        });

        let agent = build_agent(Duration::from_secs(2));
        let headers = vec![("Server".to_string(), "unit-test".to_string())];
        let contains = Some("hello".to_string());
        let status =
            check_with_retries(&agent, &format!("{}/ok", server.base_url()), &headers, &contains, 0);

        assert!(status.status.is_ok());
        assert!(status.response_time.as_millis() > 0);
    }

    #[test]
    fn test_header_mismatch() {
        let server = MockServer::start();

        let _m = server.mock(|when, then| {
            when.method(GET).path("/h");
            then.status(200)
                .header("Server", "unit-test")
                .body("ok");
        });

        let agent = build_agent(Duration::from_secs(2));
        let headers = vec![("Server".to_string(), "expected".to_string())];
        let status =
            check_with_retries(&agent, &format!("{}/h", server.base_url()), &headers, &None, 0);

        assert!(status.status.is_err());
        let msg = status.status.err().unwrap();
        assert!(msg.contains("header mismatch"));
    }

    #[test]
    fn test_body_contains_validation() {
        let server = MockServer::start();

        let _m = server.mock(|when, then| {
            when.method(GET).path("/b");
            then.status(200).body("foo bar baz");
        });

        let agent = build_agent(Duration::from_secs(2));
        let status =
            check_with_retries(&agent, &format!("{}/b", server.base_url()), &[], &Some("bar".into()), 0);

        assert!(status.status.is_ok());

        let status_fail =
            check_with_retries(&agent, &format!("{}/b", server.base_url()), &[], &Some("nope".into()), 0);
        assert!(status_fail.status.is_err());
    }

    #[test]
    fn test_timeout_error() {
        let server = MockServer::start();

        let _m = server.mock(|when, then| {
            when.method(GET).path("/slow");
            then.status(200)
                .delay(std::time::Duration::from_secs(3)) // server delays response
                .body("slow");
        });

        let agent = build_agent(Duration::from_secs(1)); // 1s timeout -> should time out
        let status =
            check_with_retries(&agent, &format!("{}/slow", server.base_url()), &[], &None, 0);
        assert!(status.status.is_err());
        let msg = status.status.err().unwrap();
        assert!(msg.contains("error"));
    }

    #[test]
    fn test_concurrency_50() {
        let server = MockServer::start();

        // Create 50 endpoints
        for i in 0..50 {
            let path = format!("/ok{i}");
            server.mock(|when, then| {
                when.method(GET).path(path.clone());
                then.status(200).body("ok");
            });
        }

        let urls: Vec<String> = (0..50)
            .map(|i| format!("{}/ok{i}", server.base_url()))
            .collect();

        // Build config-like items
        let agent = build_agent(Duration::from_secs(2));

        // Spawn 50 threads to simulate concurrency for this test
        let (tx, rx) = std::sync::mpsc::channel::<WebsiteStatus>();
        for url in urls.clone() {
            let tx = tx.clone();
            let agent = agent.clone();
            std::thread::spawn(move || {
                let s = check_with_retries(&agent, &url, &[], &None, 0);
                tx.send(s).ok();
            });
        }
        drop(tx);

        let results: Vec<WebsiteStatus> = rx.into_iter().collect();
        assert_eq!(results.len(), 50);
        assert!(results.iter().all(|s| s.status.is_ok()));
    }
}
