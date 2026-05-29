use std::sync::{Arc, Mutex};
use std::time::Duration;

use slint::{Timer, TimerMode};
use slint_backend_kindle::WakeSchedule;

slint::include_modules!();

static LIBERATION_SANS: &[u8] = include_bytes!("../fonts/LiberationSans-Regular.ttf");

const CAT_FACTS_URL: &str = "https://catfact.ninja/fact";
const MAX_FETCH_ATTEMPTS: u32 = 6;
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);
const RETRY_DELAY: Duration = Duration::from_secs(2);
const MAX_LOG_LINES: usize = 12;

#[derive(serde::Deserialize)]
struct CatFact {
    fact: String,
}

struct Logger {
    lines: Vec<String>,
}

impl Logger {
    fn new() -> Self {
        Self { lines: Vec::new() }
    }

    fn push(&mut self, line: impl Into<String>) {
        let stamp = chrono::Local::now().format("%H:%M:%S");
        self.lines.push(format!("[{stamp}] {}", line.into()));
        while self.lines.len() > MAX_LOG_LINES {
            self.lines.remove(0);
        }
    }

    fn text(&self) -> String {
        self.lines.join("\n")
    }
}

/// Report the first wireless/ethernet interface's link state. Wifi typically
/// transitions `down` → `up` over 5–15 s after a resume; a probe in the log
/// helps explain why early fetch attempts may fail.
fn link_state() -> String {
    for iface in ["wlan0", "wlan1", "eth0"] {
        let path = format!("/sys/class/net/{iface}/operstate");
        if let Ok(state) = std::fs::read_to_string(&path) {
            return format!("{iface}={}", state.trim());
        }
    }
    "no-iface".to_string()
}

fn fetch_cat_fact() -> Result<String, String> {
    let response = ureq::get(CAT_FACTS_URL)
        .timeout(FETCH_TIMEOUT)
        .call()
        .map_err(|e| format!("{e}"))?;
    let body: CatFact = response.into_json().map_err(|e| format!("parse: {e}"))?;
    Ok(body.fact)
}

fn push_log(log: &Arc<Mutex<Logger>>, weak: &slint::Weak<AppWindow>, line: String) {
    let text = {
        let mut logger = log.lock().expect("log poisoned");
        logger.push(line);
        logger.text()
    };
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(app) = weak.upgrade() {
            app.set_log_text(text.into());
        }
    });
}

fn set_fact(weak: &slint::Weak<AppWindow>, fact: String) {
    let weak = weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(app) = weak.upgrade() {
            app.set_fact_text(fact.into());
        }
    });
}

/// Run the fetch on a background thread so the UI stays responsive while we
/// wait for wifi to reconnect and retry. Each attempt logs the current link
/// state — that's the signal you're looking for when verifying the wake path.
fn spawn_fetch(log: Arc<Mutex<Logger>>, weak: slint::Weak<AppWindow>) {
    std::thread::spawn(move || {
        for attempt in 1..=MAX_FETCH_ATTEMPTS {
            push_log(
                &log,
                &weak,
                format!("fetch #{attempt} ({})", link_state()),
            );
            match fetch_cat_fact() {
                Ok(fact) => {
                    push_log(&log, &weak, format!("ok: {} chars", fact.len()));
                    set_fact(&weak, fact);
                    return;
                }
                Err(e) => {
                    let trimmed: String = e.chars().take(50).collect();
                    push_log(&log, &weak, format!("err: {trimmed}"));
                    if attempt < MAX_FETCH_ATTEMPTS {
                        std::thread::sleep(RETRY_DELAY);
                    }
                }
            }
        }
        push_log(&log, &weak, "giving up".into());
    });
}

fn main() {
    let backend =
        slint_backend_kindle::install(LIBERATION_SANS).expect("failed to install Kindle backend");
    let app = AppWindow::new().expect("failed to create window");
    app.on_quit(|| std::process::exit(0));

    let log = Arc::new(Mutex::new(Logger::new()));
    push_log(&log, &app.as_weak(), "startup".into());

    let tick = {
        let weak = app.as_weak();
        move || {
            let Some(app) = weak.upgrade() else { return };
            let now = chrono::Local::now();
            app.set_time_text(now.format("%H:%M:%S").to_string().into());
        }
    };
    tick();

    let timer = Timer::default();
    timer.start(TimerMode::Repeated, Duration::from_secs(1), tick);

    // Initial fetch so we don't show an empty fact area before the first wake.
    spawn_fetch(log.clone(), app.as_weak());

    // 30s awake window gives wifi time to reconnect and the fetch to retry a
    // few times before we suspend again. Wake every 60s.
    backend.set_wake_schedule(Some(WakeSchedule {
        wake_interval: Duration::from_secs(60),
        stay_awake: Duration::from_secs(30),
    }));

    backend.on_wake({
        let log = log.clone();
        let weak = app.as_weak();
        move || {
            // We're on the UI thread here, so update directly — no need to
            // bounce through invoke_from_event_loop.
            let text = {
                let mut logger = log.lock().expect("log poisoned");
                logger.push("wake");
                logger.text()
            };
            if let Some(app) = weak.upgrade() {
                let now = chrono::Local::now();
                app.set_time_text(now.format("%H:%M:%S").to_string().into());
                app.set_log_text(text.into());
            }
            spawn_fetch(log.clone(), weak.clone());
        }
    });

    app.run().expect("event loop error");
}
