//! Availability checks. A name is only ever reported as available when the
//! registry itself says so:
//!
//! 1. DNS (over HTTPS): a domain with name servers is registered. Cheap, and
//!    it settles most taken names without bothering a registry.
//! 2. RDAP at the registry: 404 means "no such registration".
//! 3. WHOIS at the registry, for TLDs that have no RDAP service.
//!
//! Before a registry lookup is trusted for a TLD it is calibrated: it has to
//! call a known registered domain "taken" and a gibberish one "available".

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use futures::future::join_all;
use rand::RngExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{Mutex, OnceCell};
use tokio::time::{Instant, sleep_until};

use crate::cache::Cache;
use crate::net::{Request, Web, Whois};

const DOH_GOOGLE: &str = "https://dns.google/resolve";
const DOH_CLOUDFLARE: &str = "https://cloudflare-dns.com/dns-query";
const IANA_WHOIS: &str = "whois.iana.org";
/// RDAP services that exist but are missing from IANA's bootstrap registry.
/// `{tld}` is filled in; every candidate still has to pass calibration.
const RDAP_GUESSES: &[&str] = &[
    "https://rdap.identitydigital.services/rdap/",
    "https://rdap.nic.{tld}/",
];
/// Brands that get registered defensively in practically every TLD that is
/// open to the public. Used to tell live TLDs from closed ones, and as a
/// domain that is certainly taken when calibrating a registry lookup.
const LANDMARKS: &[&str] = &[
    "google",
    "amazon",
    "facebook",
    "apple",
    "microsoft",
    "netflix",
    "youtube",
    "instagram",
    "twitter",
    "paypal",
    "ebay",
    "adobe",
    "samsung",
    "nike",
    "ikea",
    "lego",
];
const LANDMARKS_NEEDED: usize = 3;

const METHOD_TTL_SECS: i64 = 28 * 24 * 3600;
const REJECTED_TTL_SECS: i64 = 3 * 24 * 3600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Available,
    Taken,
    Unknown,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Available => "available",
            Status::Taken => "taken",
            Status::Unknown => "unknown",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "available" => Status::Available,
            "taken" => Status::Taken,
            _ => Status::Unknown,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Verdict {
    pub status: Status,
    /// Who answered: "dns", "rdap", "whois" or "none".
    pub source: &'static str,
    pub detail: String,
    /// Unknown because of a hiccup (timeout, rate limit) that may pass.
    hiccup: bool,
}

impl Verdict {
    fn new(status: Status, source: &'static str, detail: impl Into<String>) -> Self {
        Self {
            status,
            source,
            detail: detail.into(),
            hiccup: false,
        }
    }

    fn hiccup(source: &'static str, detail: impl Into<String>) -> Self {
        Self {
            status: Status::Unknown,
            source,
            detail: detail.into(),
            hiccup: true,
        }
    }
}

/// How a TLD's registry can be asked about a domain.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "via", rename_all = "lowercase")]
pub enum Method {
    Rdap {
        base: String,
    },
    Whois {
        server: String,
    },
    /// The TLD cannot be verified (closed, no registry lookup, ...).
    Rejected {
        reason: String,
    },
}

impl Method {
    pub fn is_usable(&self) -> bool {
        !matches!(self, Method::Rejected { .. })
    }

    pub fn describe(&self) -> String {
        match self {
            Method::Rdap { base } => format!("RDAP {base}"),
            Method::Whois { server } => format!("WHOIS {server}"),
            Method::Rejected { reason } => format!("skipped: {reason}"),
        }
    }
}

pub struct Checker {
    web: Arc<dyn Web>,
    port43: Arc<dyn Whois>,
    cache: Arc<Cache>,
    /// RDAP base URL per TLD, from IANA's bootstrap registry.
    bootstrap: HashMap<String, String>,
    methods: Mutex<HashMap<String, Arc<OnceCell<Method>>>>,
    throttle: Throttle,
}

impl Checker {
    pub fn new(
        web: Arc<dyn Web>,
        port43: Arc<dyn Whois>,
        cache: Arc<Cache>,
        bootstrap: HashMap<String, String>,
    ) -> Self {
        Self {
            web,
            port43,
            cache,
            bootstrap,
            methods: Mutex::new(HashMap::new()),
            throttle: Throttle::default(),
        }
    }

    /// Is `domain` (a name directly under `tld`) still free?
    pub async fn check(&self, domain: &str, tld: &str) -> Verdict {
        if let Ok(Dns::Delegated) = self.dns(domain).await {
            return Verdict::new(Status::Taken, "dns", "has name servers");
        }
        match self.method(tld).await {
            Method::Rdap { base } => self.rdap(&base, domain).await,
            Method::Whois { server } => self.whois(&server, domain).await,
            Method::Rejected { reason } => Verdict::new(Status::Unknown, "none", reason),
        }
    }

    /// How long a new lookup would have to queue at this TLD's registry.
    /// Zero when unknown: this is only a hint to spread the load.
    pub fn registry_wait(&self, tld: &str) -> Duration {
        let host = match self.known_method(tld) {
            Some(Method::Rdap { base }) => host_of(&base),
            Some(Method::Whois { server }) => server,
            _ => return Duration::ZERO,
        };
        self.throttle.queue_time(&host)
    }

    /// The registry lookup for a TLD when it has been resolved already,
    /// without waiting for it.
    pub fn known_method(&self, tld: &str) -> Option<Method> {
        self.methods.try_lock().ok()?.get(tld)?.get().cloned()
    }

    /// The (calibrated) registry lookup for a TLD, resolved once per session
    /// and remembered in the cache between sessions.
    pub async fn method(&self, tld: &str) -> Method {
        let cell = {
            let mut methods = self.methods.lock().await;
            methods.entry(tld.to_string()).or_default().clone()
        };
        cell.get_or_init(|| self.resolve_method(tld)).await.clone()
    }

    async fn resolve_method(&self, tld: &str) -> Method {
        let key = format!("method:{tld}");
        let read = |max_age| self.cache.kv_get(&key, max_age).ok().flatten();
        let cached =
            read(METHOD_TTL_SECS).and_then(|json| serde_json::from_str::<Method>(&json).ok());
        if let Some(method) = cached {
            // A rejection is revisited sooner than a working lookup.
            if method.is_usable() || read(REJECTED_TTL_SECS).is_some() {
                return method;
            }
        }
        let (method, definitive) = self.discover_method(tld).await;
        if definitive && let Ok(json) = serde_json::to_string(&method) {
            let _ = self.cache.kv_put(&key, &json);
        }
        method
    }

    /// Returns the method plus whether the outcome is worth remembering
    /// (a network hiccup is not).
    async fn discover_method(&self, tld: &str) -> (Method, bool) {
        let reject = |reason: &str| Method::Rejected {
            reason: reason.to_string(),
        };

        let landmark = match self.find_landmark(tld).await {
            Ok(Some(domain)) => domain,
            Ok(None) => return (reject("names cannot be registered there directly"), true),
            Err(_) => return (reject("could not be reached"), false),
        };
        let gibberish = format!("{}.{tld}", gibberish());
        match self.dns(&gibberish).await {
            // Every name resolves here, so DNS says nothing about registration.
            Ok(Dns::Delegated) => return (reject("every name looks taken there"), true),
            Ok(_) => {}
            Err(_) => return (reject("could not be reached"), false),
        }

        // IANA's registry is authoritative; guessed servers get one quick try.
        let (rdap_bases, attempts): (Vec<String>, u32) = match self.bootstrap.get(tld) {
            Some(base) => (vec![base.clone()], 4),
            None => (
                RDAP_GUESSES
                    .iter()
                    .map(|g| g.replace("{tld}", tld))
                    .collect(),
                1,
            ),
        };
        for base in rdap_bases {
            let base = if base.ends_with('/') {
                base
            } else {
                format!("{base}/")
            };
            let taken = self.rdap_with(&base, &landmark, attempts).await;
            let free = match taken.status {
                Status::Taken => self.rdap_with(&base, &gibberish, attempts).await,
                _ => taken.clone(),
            };
            if taken.status == Status::Taken && free.status == Status::Available {
                return (Method::Rdap { base }, true);
            }
            // A throttled official server is a hiccup, not a reason to give up on it.
            if self.bootstrap.contains_key(tld) && free.hiccup {
                return (reject("not answering right now"), false);
            }
        }

        match self.whois_server(tld).await {
            Ok(Some(server)) => {
                let taken = self.whois(&server, &landmark).await;
                let free = match taken.status {
                    Status::Taken => self.whois(&server, &gibberish).await,
                    _ => taken.clone(),
                };
                if taken.status == Status::Taken && free.status == Status::Available {
                    return (Method::Whois { server }, true);
                }
                if free.hiccup {
                    return (reject("not answering right now"), false);
                }
                (reject("its answers cannot be trusted"), true)
            }
            Ok(None) => (reject("has no way to look names up"), true),
            Err(_) => (reject("could not reach IANA"), false),
        }
    }

    /// A domain in this TLD that is certainly registered, or `None` when the
    /// TLD does not look open to the public at all.
    async fn find_landmark(&self, tld: &str) -> Result<Option<String>> {
        let domains: Vec<String> = LANDMARKS.iter().map(|l| format!("{l}.{tld}")).collect();
        let results = join_all(domains.iter().map(|d| self.dns(d))).await;
        if results.iter().all(Result::is_err) {
            return Err(anyhow!("DNS over HTTPS is unreachable"));
        }
        // Broken name servers still mean somebody registered the name.
        let registered = results
            .iter()
            .filter(|r| matches!(r, Ok(Dns::Delegated | Dns::Broken)))
            .count();
        let reference = domains
            .iter()
            .zip(&results)
            .find(|(_, r)| matches!(r, Ok(Dns::Delegated)))
            .map(|(domain, _)| domain.clone());
        Ok(reference.filter(|_| registered >= LANDMARKS_NEEDED))
    }

    // --- DNS ---------------------------------------------------------------

    /// Asks a public resolver for the name servers of a domain. Only
    /// `Delegated` is proof of anything: that the domain is registered.
    async fn dns(&self, domain: &str) -> Result<Dns> {
        let mut last_error = anyhow!("no resolver tried");
        for resolver in [DOH_GOOGLE, DOH_CLOUDFLARE] {
            let request = Request::get(resolver)
                .query("name", domain)
                .query("type", "NS")
                .header("accept", "application/dns-json")
                .timeout(Duration::from_secs(6));
            last_error = match self.web.get(request).await {
                Ok(reply) if reply.is_success() => match reply.json() {
                    Some(body) => return Ok(read_dns_answer(&body)),
                    None => anyhow!("{resolver} answered with something unreadable"),
                },
                Ok(reply) => anyhow!("{resolver} answered HTTP {}", reply.status),
                Err(reason) => anyhow!("{resolver}: {reason}"),
            };
        }
        Err(last_error)
    }

    // --- RDAP --------------------------------------------------------------

    async fn rdap(&self, base: &str, domain: &str) -> Verdict {
        self.rdap_with(base, domain, 4).await
    }

    async fn rdap_with(&self, base: &str, domain: &str, attempts: u32) -> Verdict {
        let url = format!("{base}domain/{domain}");
        let host = host_of(base);
        let mut detail = String::from("rate limited");
        for _ in 0..attempts {
            self.throttle.wait(&host, rdap_gap(&host)).await;
            let request = Request::get(&url)
                .header("accept", "application/rdap+json, application/json")
                .timeout(Duration::from_secs(if attempts == 1 { 5 } else { 10 }));
            let reply = match self.web.get(request).await {
                Ok(reply) => reply,
                Err(reason) => {
                    detail = format!("lookup failed: {reason}");
                    continue;
                }
            };
            match reply.status {
                200 => {
                    // A few servers wrap "not found" in a 200.
                    let body = reply.json().unwrap_or(Value::Null);
                    if body.get("errorCode").and_then(Value::as_u64) == Some(404) {
                        return Verdict::new(Status::Available, "rdap", "no registration found");
                    }
                    return Verdict::new(Status::Taken, "rdap", "registered");
                }
                404 => return Verdict::new(Status::Available, "rdap", "no registration found"),
                429 | 503 => {
                    self.throttle.slow_down(&host, reply.retry_after).await;
                    detail = "asked too often, waiting a bit".to_string();
                }
                code => {
                    return Verdict::new(
                        Status::Unknown,
                        "rdap",
                        format!("answered with error {code}"),
                    );
                }
            }
        }
        Verdict::hiccup("rdap", detail)
    }

    // --- WHOIS -------------------------------------------------------------

    async fn whois(&self, server: &str, domain: &str) -> Verdict {
        self.throttle
            .wait(server, Duration::from_millis(1200))
            .await;
        let query = match server {
            // DENIC only shows the status when asked for the domain object.
            "whois.denic.de" => format!("-T dn {domain}"),
            _ => domain.to_string(),
        };
        match self.port43.ask(server, &query).await {
            Ok(text) => match classify_whois(&text) {
                WhoisAnswer::Free => {
                    Verdict::new(Status::Available, "whois", "no registration found")
                }
                WhoisAnswer::Registered => Verdict::new(Status::Taken, "whois", "registered"),
                WhoisAnswer::Unclear => {
                    Verdict::new(Status::Unknown, "whois", "the answer was unclear")
                }
                WhoisAnswer::Throttled => {
                    self.throttle.slow_down(server, None).await;
                    Verdict::hiccup("whois", "asked too often, waiting a bit")
                }
            },
            Err(e) => Verdict::hiccup("whois", format!("lookup failed: {e}")),
        }
    }

    /// The WHOIS server IANA lists for a TLD.
    async fn whois_server(&self, tld: &str) -> Result<Option<String>> {
        self.throttle
            .wait(IANA_WHOIS, Duration::from_millis(300))
            .await;
        let text = self.port43.ask(IANA_WHOIS, tld).await?;
        Ok(text
            .lines()
            .find_map(|line| line.strip_prefix("whois:"))
            .map(|server| server.trim().to_string())
            .filter(|server| !server.is_empty()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dns {
    /// The domain has name servers.
    Delegated,
    /// The lookup failed at the domain's own name servers (SERVFAIL).
    Broken,
    /// No delegation found. Says nothing: registered domains can lack one.
    Nothing,
}

fn read_dns_answer(body: &Value) -> Dns {
    const NS: u64 = 2;
    const SERVFAIL: u64 = 2;
    let has_ns = body
        .get("Answer")
        .and_then(Value::as_array)
        .is_some_and(|answers| {
            answers
                .iter()
                .any(|a| a.get("type").and_then(Value::as_u64) == Some(NS))
        });
    match body.get("Status").and_then(Value::as_u64) {
        Some(0) if has_ns => Dns::Delegated,
        Some(SERVFAIL) => Dns::Broken,
        _ => Dns::Nothing,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhoisAnswer {
    Free,
    Registered,
    /// The registry refuses to answer for now.
    Throttled,
    /// Nothing recognisable, or signs of both.
    Unclear,
}

/// Read a registry's WHOIS answer. Registries all phrase this differently, so
/// anything that is not clearly one or the other is not a verdict.
pub fn classify_whois(text: &str) -> WhoisAnswer {
    // Specific on purpose: footers like "access is rate limited" are boilerplate.
    const THROTTLED: &[&str] = &[
        "limit exceeded",
        "quota exceeded",
        "queries exceeded",
        "too many requests",
        "too many queries",
        "too many connections",
        "try again later",
        "access denied",
    ];
    const FREE: &[&str] = &[
        "no match",
        "not found",
        "no entries found",
        "no data found",
        "nothing found",
        "no object found",
        "no such domain",
        "not registered",
        "does not exist",
        "status: free",
        "status: available",
        "is available",
        "is free",
        "available for registration",
    ];
    const REGISTERED: &[&str] = &[
        "registrar:",
        "creation date:",
        "created:",
        "registered on:",
        "registered:",
        "expiry date:",
        "expires:",
        "nserver:",
        "name server:",
        "nameserver:",
        "nameservers:",
        "status: connect",
        "status: active",
        "status: ok",
        "status: registered",
        "status: not available",
    ];
    let text = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| text.contains(n));
    match (has(FREE), has(REGISTERED)) {
        _ if has(THROTTLED) => WhoisAnswer::Throttled,
        (true, false) => WhoisAnswer::Free,
        (false, true) => WhoisAnswer::Registered,
        _ => WhoisAnswer::Unclear,
    }
}

fn gibberish() -> String {
    let mut rng = rand::rng();
    let tail: String = (0..14)
        .map(|_| rng.random_range(b'a'..=b'z') as char)
        .collect();
    format!("qzx{tail}")
}

fn host_of(url: &str) -> String {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or(url)
        .to_string()
}

/// Time between two lookups at the same registry. SIDN (.nl) and Google
/// (.app, .dev, ...) answer 429 when asked more than about once a second;
/// other registries that complain are slowed down as we go.
fn rdap_gap(host: &str) -> Duration {
    match host {
        "rdap.sidn.nl" | "pubapi.registry.google" => SLOW_GAP,
        _ => Duration::from_millis(350),
    }
}

const SLOW_GAP: Duration = Duration::from_millis(1500);
const SLOWEST_GAP: Duration = Duration::from_secs(8);

/// Spaces out requests per host, and backs off when a host complains.
#[derive(Default)]
struct Throttle {
    hosts: Mutex<HashMap<String, Slot>>,
}

struct Slot {
    next: Instant,
    gap: Duration,
}

impl Throttle {
    async fn wait(&self, host: &str, gap: Duration) {
        let turn = {
            let mut hosts = self.hosts.lock().await;
            let now = Instant::now();
            let slot = hosts
                .entry(host.to_string())
                .or_insert(Slot { next: now, gap });
            let turn = slot.next.max(now);
            slot.next = turn + slot.gap;
            turn
        };
        sleep_until(turn).await;
    }

    /// The host complained: ask less often from now on and give it a break.
    async fn slow_down(&self, host: &str, retry_after_secs: Option<u64>) {
        let mut hosts = self.hosts.lock().await;
        let now = Instant::now();
        let slot = hosts.entry(host.to_string()).or_insert(Slot {
            next: now,
            gap: SLOW_GAP,
        });
        slot.gap = (slot.gap * 2).clamp(SLOW_GAP, SLOWEST_GAP);
        let pause = Duration::from_secs(retry_after_secs.unwrap_or(5).clamp(5, 30));
        slot.next = slot.next.max(now + pause);
    }

    fn queue_time(&self, host: &str) -> Duration {
        let Ok(hosts) = self.hosts.try_lock() else {
            return Duration::ZERO;
        };
        hosts.get(host).map_or(Duration::ZERO, |slot| {
            slot.next.saturating_duration_since(Instant::now())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::fake;

    const HAS_NAME_SERVERS: &str = r#"{"Status":0,"Answer":[{"type":2,"data":"ns1.example."}]}"#;
    const NO_SUCH_NAME: &str = r#"{"Status":3}"#;

    /// A checker on a scripted web and WHOIS, with a cache in memory.
    struct World {
        web: Arc<fake::Web>,
        whois: Arc<fake::Whois>,
        clock: Arc<fake::Clock>,
        cache: Arc<Cache>,
    }

    impl World {
        fn new() -> Self {
            let clock = Arc::new(fake::Clock::at(1_000));
            Self {
                web: Arc::new(fake::Web::new()),
                whois: Arc::new(fake::Whois::new()),
                cache: Arc::new(Cache::in_memory(clock.clone())),
                clock,
            }
        }

        /// A checker that knows these RDAP services from IANA.
        fn checker(&self, bootstrap: &[(&str, &str)]) -> Checker {
            let bootstrap = bootstrap
                .iter()
                .map(|(tld, base)| (tld.to_string(), base.to_string()))
                .collect();
            Checker::new(
                self.web.clone(),
                self.whois.clone(),
                self.cache.clone(),
                bootstrap,
            )
        }

        /// The registry lookup of a TLD is known already.
        fn knows(&self, tld: &str, method: &Method) {
            let json = serde_json::to_string(method).unwrap();
            self.cache.kv_put(&format!("method:{tld}"), &json).unwrap();
        }

        /// A TLD that is open to the public: the big brands are all there,
        /// and a name nobody would register is not.
        fn open_tld(&self, tld: &str) {
            for landmark in LANDMARKS {
                let name = format!("name={landmark}.{tld}&");
                self.web.on(&name, 200, HAS_NAME_SERVERS);
            }
            self.web.on("name=qzx", 200, NO_SUCH_NAME);
        }
    }

    fn rdap(base: &str) -> Method {
        Method::Rdap {
            base: base.to_string(),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn name_servers_settle_it_without_a_registry() {
        let world = World::new();
        world.web.on("name=papega.ai", 200, HAS_NAME_SERVERS);
        let verdict = world.checker(&[]).check("papega.ai", "ai").await;
        assert_eq!((verdict.status, verdict.source), (Status::Taken, "dns"));
        // One question, to one resolver.
        assert_eq!(world.web.asked().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn the_registry_has_the_last_word() {
        let world = World::new();
        world.knows("ing", &rdap("https://rdap.example/"));
        world.web.on("name=", 200, NO_SUCH_NAME);
        world
            .web
            .on("domain/taken.ing", 200, r#"{"objectClassName":"domain"}"#);
        world
            .web
            .on("domain/wrapped.ing", 200, r#"{"errorCode":404}"#);
        world.web.on("domain/broken.ing", 500, "");
        let checker = world.checker(&[]);

        // No such registration: the only way a name is ever called available.
        let free = checker.check("cook.ing", "ing").await;
        assert_eq!((free.status, free.source), (Status::Available, "rdap"));
        assert_eq!(
            checker.check("taken.ing", "ing").await.status,
            Status::Taken
        );
        // A few servers say "not found" with a 200.
        assert_eq!(
            checker.check("wrapped.ing", "ing").await.status,
            Status::Available
        );
        // An error is not a verdict.
        let broken = checker.check("broken.ing", "ing").await;
        assert_eq!(broken.status, Status::Unknown);
        assert!(broken.detail.contains("500"), "{}", broken.detail);
        // The registry is asked the way RDAP wants it.
        let requests = world.web.requests();
        let asked = requests
            .iter()
            .find(|r| r.url.ends_with("domain/cook.ing"))
            .unwrap();
        assert_eq!(asked.url, "https://rdap.example/domain/cook.ing");
        assert!(
            asked
                .headers
                .iter()
                .any(|(name, value)| *name == "accept" && value.contains("rdap+json"))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_busy_registry_is_given_time_and_asked_again() {
        let world = World::new();
        world.knows("app", &rdap("https://rdap.example/"));
        world.web.on("name=", 200, NO_SUCH_NAME);
        world
            .web
            .busy("domain/sn.app", 429, 2)
            .on("domain/sn.app", 404, "");
        let started = Instant::now();
        let verdict = world.checker(&[]).check("sn.app", "app").await;
        assert_eq!(verdict.status, Status::Available);
        assert_eq!(world.web.count("domain/sn.app"), 2);
        assert!(started.elapsed() >= Duration::from_secs(2));

        // One that stays busy is a shrug, not a verdict.
        world.web.busy("domain/tr.app", 429, 1);
        let shrug = world.checker(&[]).check("tr.app", "app").await;
        assert_eq!(shrug.status, Status::Unknown);
        assert!(shrug.hiccup);
    }

    #[tokio::test(start_paused = true)]
    async fn a_registry_that_cannot_be_reached_is_a_shrug() {
        let world = World::new();
        world.knows("ing", &rdap("https://rdap.example/"));
        world.web.on("name=", 200, NO_SUCH_NAME);
        world.web.down("rdap.example");
        let verdict = world.checker(&[]).check("cook.ing", "ing").await;
        assert_eq!(verdict.status, Status::Unknown);
        assert_eq!(verdict.detail, "lookup failed: cannot connect");
    }

    #[tokio::test(start_paused = true)]
    async fn whois_answers_are_read_with_care() {
        let world = World::new();
        let server = "whois.example";
        world.knows(
            "st",
            &Method::Whois {
                server: server.into(),
            },
        );
        world.knows(
            "de",
            &Method::Whois {
                server: "whois.denic.de".into(),
            },
        );
        world.web.on("name=", 200, NO_SUCH_NAME);
        world
            .whois
            .on(server, "win.st", "No match for domain WIN.ST");
        world
            .whois
            .on(server, "kun.st", "Domain Name: KUN.ST\nRegistrar: Example");
        world.whois.on(server, "odd.st", "Something else entirely");
        world.whois.on(server, "fast.st", "Query limit exceeded");
        world.whois.on(
            "whois.denic.de",
            "-T dn frei.de",
            "Domain: frei.de\nStatus: free",
        );
        let checker = world.checker(&[]);

        let free = checker.check("win.st", "st").await;
        assert_eq!((free.status, free.source), (Status::Available, "whois"));
        assert_eq!(checker.check("kun.st", "st").await.status, Status::Taken);
        assert_eq!(checker.check("odd.st", "st").await.status, Status::Unknown);
        let throttled = checker.check("fast.st", "st").await;
        assert!(throttled.hiccup);
        // A server that does not answer at all.
        let silent = checker.check("gone.st", "st").await;
        assert_eq!(silent.status, Status::Unknown);
        assert!(
            silent.detail.starts_with("lookup failed"),
            "{}",
            silent.detail
        );
        // DENIC only tells when asked for the domain object.
        assert_eq!(
            checker.check("frei.de", "de").await.status,
            Status::Available
        );
        let denic = ("whois.denic.de".to_string(), "-T dn frei.de".to_string());
        assert!(world.whois.asked().contains(&denic));
    }

    #[tokio::test(start_paused = true)]
    async fn a_registry_is_tried_out_before_it_is_trusted() {
        let world = World::new();
        world.open_tld("ing");
        // It calls a famous name taken, and nonsense available: trusted.
        world.web.on("rdap.example/domain/google.ing", 200, "{}");
        let checker = world.checker(&[("ing", "https://rdap.example")]);
        assert_eq!(checker.method("ing").await, rdap("https://rdap.example/"));
        // Remembered for this session, and for the next.
        let asked = world.web.asked().len();
        assert_eq!(checker.method("ing").await, rdap("https://rdap.example/"));
        assert_eq!(world.web.asked().len(), asked);
        assert_eq!(
            checker.known_method("ing"),
            Some(rdap("https://rdap.example/"))
        );
        assert_eq!(
            world.checker(&[]).method("ing").await,
            rdap("https://rdap.example/")
        );
        assert_eq!(world.web.asked().len(), asked);
    }

    #[tokio::test(start_paused = true)]
    async fn a_registry_that_calls_everything_taken_is_not_trusted() {
        let world = World::new();
        world.open_tld("xx");
        // Whatever is asked, it says "registered".
        world.web.on("rdap.example/domain/", 200, "{}");
        world
            .whois
            .on("whois.iana.org", "xx", "domain: XX\nstatus: ACTIVE");
        let checker = world.checker(&[("xx", "https://rdap.example/")]);
        let method = checker.method("xx").await;
        assert_eq!(
            method,
            Method::Rejected {
                reason: "has no way to look names up".into()
            }
        );
        // And a name there is never called available.
        let verdict = checker.check("word.xx", "xx").await;
        assert_eq!((verdict.status, verdict.source), (Status::Unknown, "none"));
    }

    #[tokio::test(start_paused = true)]
    async fn without_rdap_the_whois_server_of_iana_is_tried() {
        let world = World::new();
        world.open_tld("st");
        world.whois.on(
            "whois.iana.org",
            "st",
            "domain: ST\nwhois:        whois.nic.st\nstatus: ACTIVE",
        );
        world.whois.on(
            "whois.nic.st",
            "google.st",
            "Domain Name: GOOGLE.ST\nRegistrar: MarkMonitor",
        );
        world.whois.on("whois.nic.st", "qzx", "No match for domain");
        let method = world.checker(&[]).method("st").await;
        assert_eq!(
            method,
            Method::Whois {
                server: "whois.nic.st".into()
            }
        );
    }

    #[tokio::test(start_paused = true)]
    async fn extensions_that_cannot_be_checked_are_told_apart() {
        // Nobody registered the big brands: not open to the public.
        let closed = World::new();
        closed.web.on("name=", 200, NO_SUCH_NAME);
        let reason = |method: Method| match method {
            Method::Rejected { reason } => reason,
            other => panic!("{other:?}"),
        };
        assert_eq!(
            reason(closed.checker(&[]).method("zz").await),
            "names cannot be registered there directly"
        );
        // Every name has name servers: DNS says nothing there.
        let wildcard = World::new();
        wildcard.web.on("name=", 200, HAS_NAME_SERVERS);
        assert_eq!(
            reason(wildcard.checker(&[]).method("ws").await),
            "every name looks taken there"
        );
        // No DNS at all is a hiccup: tried again next time, not remembered.
        let offline = World::new();
        offline.web.down("dns.google").down("cloudflare-dns.com");
        assert_eq!(
            reason(offline.checker(&[]).method("ai").await),
            "could not be reached"
        );
        assert!(
            offline
                .cache
                .kv_get("method:ai", i64::MAX)
                .unwrap()
                .is_none()
        );
        assert!(
            closed
                .cache
                .kv_get("method:zz", i64::MAX)
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_rejection_is_looked_at_again_after_three_days() {
        let world = World::new();
        world.knows(
            "zz",
            &Method::Rejected {
                reason: "old news".into(),
            },
        );
        world.open_tld("zz");
        world.web.on("rdap.example/domain/google.zz", 200, "{}");
        let bootstrap = [("zz", "https://rdap.example/")];
        // Within three days the rejection stands.
        assert!(!world.checker(&bootstrap).method("zz").await.is_usable());
        world.clock.advance(REJECTED_TTL_SECS);
        assert_eq!(
            world.checker(&bootstrap).method("zz").await,
            rdap("https://rdap.example/")
        );
    }

    #[tokio::test(start_paused = true)]
    async fn lookups_at_one_registry_are_spaced_out() {
        let world = World::new();
        world.knows("nl", &rdap("https://rdap.sidn.nl/"));
        world.web.on("name=", 200, NO_SUCH_NAME);
        let checker = world.checker(&[]);
        let started = Instant::now();
        for name in ["een.nl", "twee.nl", "drie.nl"] {
            assert_eq!(checker.check(name, "nl").await.status, Status::Available);
        }
        // SIDN complains above about one question a second.
        assert!(started.elapsed() >= SLOW_GAP * 2, "{:?}", started.elapsed());
        assert!(checker.registry_wait("nl") > Duration::ZERO);
        assert_eq!(checker.registry_wait("com"), Duration::ZERO);
    }

    #[test]
    fn verdicts_and_methods_have_words() {
        assert_eq!(Status::parse(Status::Available.as_str()), Status::Available);
        assert_eq!(Status::parse(Status::Taken.as_str()), Status::Taken);
        assert_eq!(Status::parse("whatever"), Status::Unknown);
        assert_eq!(rdap("https://x/").describe(), "RDAP https://x/");
        let whois = Method::Whois {
            server: "whois.x".into(),
        };
        assert_eq!(whois.describe(), "WHOIS whois.x");
        let rejected = Method::Rejected {
            reason: "closed".into(),
        };
        assert_eq!(rejected.describe(), "skipped: closed");
        assert!(whois.is_usable() && !rejected.is_usable());
        assert!(gibberish().starts_with("qzx") && gibberish() != gibberish());
    }

    #[test]
    fn whois_answers() {
        assert_eq!(
            classify_whois("Domain not found.\n>>> Last update of WHOIS database"),
            WhoisAnswer::Free
        );
        assert_eq!(
            classify_whois("Domain: zzz.de\nStatus: free"),
            WhoisAnswer::Free
        );
        assert_eq!(
            classify_whois("Domain:\tzzz.be\nStatus:\tAVAILABLE"),
            WhoisAnswer::Free
        );
        assert_eq!(
            classify_whois("The queried object does not exist: DOMAIN NOT FOUND"),
            WhoisAnswer::Free
        );
        assert_eq!(
            classify_whois("Domain:\tdns.be\nStatus:\tNOT AVAILABLE\nRegistered:\tTue Jan 1 1999"),
            WhoisAnswer::Registered
        );
        assert_eq!(
            classify_whois("Domain: denic.de\nNserver: ns1.denic.de\nStatus: connect"),
            WhoisAnswer::Registered
        );
        // Boilerplate about rate limits is not a refusal.
        assert_eq!(
            classify_whois(
                "Domain Name: GOOGLE.CO\nRegistrar: MarkMonitor\n\nAccess to the Whois and RDAP services is rate limited."
            ),
            WhoisAnswer::Registered
        );
        // Contradictory, throttled or empty answers are never a verdict.
        assert_eq!(
            classify_whois("Registrar: X\nDNSSEC: not found"),
            WhoisAnswer::Unclear
        );
        assert_eq!(
            classify_whois("Query limit exceeded, not found"),
            WhoisAnswer::Throttled
        );
        assert_eq!(classify_whois(""), WhoisAnswer::Unclear);
    }

    #[test]
    fn dns_answers() {
        let read = |json: &str| read_dns_answer(&serde_json::from_str(json).unwrap());
        assert_eq!(
            read(
                r#"{"Status":0,"Answer":[{"name":"papega.ai.","type":2,"data":"ns1.afternic.com."}]}"#
            ),
            Dns::Delegated
        );
        assert_eq!(
            read(r#"{"Status":3,"Authority":[{"name":"io.","type":6}]}"#),
            Dns::Nothing
        );
        assert_eq!(
            read(r#"{"Status":0,"Authority":[{"name":"name.","type":6}]}"#),
            Dns::Nothing
        );
        assert_eq!(read(r#"{"Status":2}"#), Dns::Broken);
    }

    #[test]
    fn hosts() {
        assert_eq!(host_of("https://rdap.sidn.nl/"), "rdap.sidn.nl");
        assert_eq!(
            host_of("https://rdap.identitydigital.services/rdap/"),
            "rdap.identitydigital.services"
        );
    }
}
