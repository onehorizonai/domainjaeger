//! The outside world, behind three small doors: the web, WHOIS and the
//! clock. The rest of the crate only ever goes through these, so a test can
//! put something predictable in their place and run without a network.

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use futures::future::BoxFuture;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// A page to get: an address, what to ask for and how long to wait.
#[derive(Debug, Clone, Default)]
pub struct Request {
    pub url: String,
    pub query: Vec<(String, String)>,
    pub headers: Vec<(&'static str, String)>,
    pub timeout: Option<Duration>,
}

impl Request {
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            ..Self::default()
        }
    }

    pub fn query(mut self, name: &str, value: impl Into<String>) -> Self {
        self.query.push((name.to_string(), value.into()));
        self
    }

    pub fn header(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.headers.push((name, value.into()));
        self
    }

    pub fn timeout(mut self, limit: Duration) -> Self {
        self.timeout = Some(limit);
        self
    }

    /// The address with everything that is asked for: `url?name=value&...`.
    #[cfg(test)]
    pub fn address(&self) -> String {
        let query: Vec<String> = self.query.iter().map(|(n, v)| format!("{n}={v}")).collect();
        match query.is_empty() {
            true => self.url.clone(),
            false => format!("{}?{}", self.url, query.join("&")),
        }
    }
}

/// What a server said.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reply {
    pub status: u16,
    /// The seconds a server asks to be left alone for (`Retry-After`).
    pub retry_after: Option<u64>,
    pub body: String,
}

impl Reply {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn is_server_error(&self) -> bool {
        (500..600).contains(&self.status)
    }

    /// The body as JSON, when it is that.
    pub fn json(&self) -> Option<Value> {
        serde_json::from_str(&self.body).ok()
    }
}

/// The web. An error is a server that could not be reached at all, in a
/// few words that can be shown: "timeout", "cannot connect".
pub trait Web: Send + Sync {
    fn get(&self, request: Request) -> BoxFuture<'_, Result<Reply, String>>;
}

/// WHOIS: one question to one server, and its answer as text.
pub trait Whois: Send + Sync {
    fn ask<'a>(&'a self, server: &'a str, query: &'a str) -> BoxFuture<'a, Result<String>>;
}

/// What time it is, in seconds since 1970.
pub trait Clock: Send + Sync {
    fn now(&self) -> i64;
}

/// The real web.
pub struct Online {
    client: reqwest::Client,
}

impl Online {
    /// `user_agent` is what every server gets to see, unless a request
    /// brings its own.
    pub fn new(user_agent: &str) -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(user_agent)
            .connect_timeout(Duration::from_secs(6))
            .timeout(Duration::from_secs(25))
            .build()?;
        Ok(Self { client })
    }
}

impl Web for Online {
    fn get(&self, request: Request) -> BoxFuture<'_, Result<Reply, String>> {
        Box::pin(async move {
            let mut call = self.client.get(&request.url).query(&request.query);
            for (name, value) in &request.headers {
                call = call.header(*name, value);
            }
            if let Some(limit) = request.timeout {
                call = call.timeout(limit);
            }
            let response = call.send().await.map_err(|e| in_short(&e))?;
            let status = response.status().as_u16();
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse().ok());
            let body = response.text().await.map_err(|e| in_short(&e))?;
            Ok(Reply {
                status,
                retry_after,
                body,
            })
        })
    }
}

fn in_short(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "timeout"
    } else if e.is_connect() {
        "cannot connect"
    } else {
        "network error"
    }
    .to_string()
}

/// The real WHOIS: a line of text to port 43, and whatever comes back.
pub struct Port43;

impl Whois for Port43 {
    fn ask<'a>(&'a self, server: &'a str, query: &'a str) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let exchange = async {
                let mut stream = TcpStream::connect((server, 43)).await?;
                stream.write_all(format!("{query}\r\n").as_bytes()).await?;
                let mut bytes = Vec::new();
                stream.take(64 * 1024).read_to_end(&mut bytes).await?;
                Ok::<_, std::io::Error>(String::from_utf8_lossy(&bytes).into_owned())
            };
            tokio::time::timeout(Duration::from_secs(8), exchange)
                .await
                .map_err(|_| anyhow!("{server} timed out"))?
                .with_context(|| format!("{server} unreachable"))
        })
    }
}

/// The real clock.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> i64 {
        chrono::Utc::now().timestamp()
    }
}

/// Stand-ins for tests: a web and a WHOIS that answer from a script, and a
/// clock that only moves when it is told to.
#[cfg(test)]
pub mod fake {
    use std::collections::{HashMap, VecDeque};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicI64, Ordering};

    use super::*;

    /// Part of an address, and what addresses with it answer, in order.
    type Rule = (String, VecDeque<Result<Reply, String>>);

    /// A web that answers from a script. The first rule whose text is part
    /// of the address answers. A rule with several answers gives them one
    /// by one and keeps repeating the last. What no rule covers does not
    /// exist: 404.
    #[derive(Default)]
    pub struct Web {
        rules: Mutex<Vec<Rule>>,
        asked: Mutex<Vec<Request>>,
    }

    impl Web {
        pub fn new() -> Self {
            Self::default()
        }

        fn rule(&self, part: &str, answer: Result<Reply, String>) -> &Self {
            let mut rules = self.rules.lock().unwrap();
            match rules.iter_mut().find(|(known, _)| known == part) {
                Some((_, answers)) => answers.push_back(answer),
                None => rules.push((part.to_string(), VecDeque::from([answer]))),
            }
            self
        }

        /// Addresses with `part` in them answer with this status and body.
        pub fn on(&self, part: &str, status: u16, body: &str) -> &Self {
            self.rule(part, Ok(reply(status, body)))
        }

        /// The same, with a server that asks to be left alone for a while.
        pub fn busy(&self, part: &str, status: u16, retry_after: u64) -> &Self {
            let reply = Reply {
                retry_after: Some(retry_after),
                ..reply(status, "")
            };
            self.rule(part, Ok(reply))
        }

        /// Addresses with `part` in them cannot be reached.
        pub fn down(&self, part: &str) -> &Self {
            self.rule(part, Err("cannot connect".to_string()))
        }

        /// Every address that was asked for, in order.
        pub fn asked(&self) -> Vec<String> {
            let asked = self.asked.lock().unwrap();
            asked.iter().map(Request::address).collect()
        }

        /// The requests themselves, to look at their headers.
        pub fn requests(&self) -> Vec<Request> {
            self.asked.lock().unwrap().clone()
        }

        /// How many requests had `part` in their address.
        pub fn count(&self, part: &str) -> usize {
            self.asked().iter().filter(|a| a.contains(part)).count()
        }
    }

    pub fn reply(status: u16, body: &str) -> Reply {
        Reply {
            status,
            retry_after: None,
            body: body.to_string(),
        }
    }

    impl super::Web for Web {
        fn get(&self, request: Request) -> BoxFuture<'_, Result<Reply, String>> {
            let address = request.address();
            self.asked.lock().unwrap().push(request);
            let mut rules = self.rules.lock().unwrap();
            let answer = match rules.iter_mut().find(|(part, _)| address.contains(part)) {
                Some((_, answers)) if answers.len() > 1 => answers.pop_front().unwrap(),
                Some((_, answers)) => answers[0].clone(),
                None => Ok(reply(404, "")),
            };
            Box::pin(async move { answer })
        }
    }

    /// WHOIS servers that answer from a script: a server answers with the
    /// text of the first rule whose question is part of what is asked. A
    /// server without a rule cannot be reached.
    #[derive(Default)]
    pub struct Whois {
        rules: Mutex<HashMap<String, Vec<(String, String)>>>,
        asked: Mutex<Vec<(String, String)>>,
    }

    impl Whois {
        pub fn new() -> Self {
            Self::default()
        }

        /// `server` answers questions with `part` in them with `text`.
        pub fn on(&self, server: &str, part: &str, text: &str) -> &Self {
            let mut rules = self.rules.lock().unwrap();
            let rule = (part.to_string(), text.to_string());
            rules.entry(server.to_string()).or_default().push(rule);
            self
        }

        /// Every (server, question) that was asked, in order.
        pub fn asked(&self) -> Vec<(String, String)> {
            self.asked.lock().unwrap().clone()
        }
    }

    impl super::Whois for Whois {
        fn ask<'a>(&'a self, server: &'a str, query: &'a str) -> BoxFuture<'a, Result<String>> {
            let asked = (server.to_string(), query.to_string());
            self.asked.lock().unwrap().push(asked);
            let rules = self.rules.lock().unwrap();
            let answer = rules
                .get(server)
                .and_then(|rules| rules.iter().find(|(part, _)| query.contains(part)))
                .map(|(_, text)| text.clone())
                .ok_or_else(|| anyhow!("{server} unreachable"));
            Box::pin(async move { answer })
        }
    }

    /// A clock that stands still until it is moved.
    pub struct Clock(AtomicI64);

    impl Clock {
        pub fn at(now: i64) -> Self {
            Self(AtomicI64::new(now))
        }

        pub fn advance(&self, seconds: i64) {
            self.0.fetch_add(seconds, Ordering::Relaxed);
        }
    }

    impl super::Clock for Clock {
        fn now(&self) -> i64 {
            self.0.load(Ordering::Relaxed)
        }
    }

    #[tokio::test]
    async fn the_scripted_web_answers_in_order_and_remembers() {
        let web = Web::new();
        web.on("example.org/a", 200, "first")
            .on("example.org/a", 200, "second");
        web.down("example.net");
        let get = |url: &str| super::Web::get(&web, Request::get(url).query("q", "1"));
        assert_eq!(get("https://example.org/a").await.unwrap().body, "first");
        assert_eq!(get("https://example.org/a").await.unwrap().body, "second");
        // The last answer keeps coming.
        assert_eq!(get("https://example.org/a").await.unwrap().body, "second");
        assert_eq!(get("https://example.org/b").await.unwrap().status, 404);
        assert!(get("https://example.net/").await.is_err());
        assert_eq!(web.count("example.org"), 4);
        assert_eq!(web.asked()[0], "https://example.org/a?q=1");
    }

    #[test]
    fn replies_know_what_they_are() {
        assert!(reply(204, "").is_success());
        assert!(!reply(404, "").is_success());
        assert!(reply(503, "").is_server_error());
        assert_eq!(reply(200, r#"{"a":1}"#).json().unwrap()["a"], 1);
        assert!(reply(200, "<html>").json().is_none());
    }
}
