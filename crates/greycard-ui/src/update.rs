//! Whether a newer release is out: one request to GitHub's releases
//! API for the repository's latest release, at launch at most once a
//! day and whenever the Settings sheet's Check now is pressed.
//!
//! The request carries the editor's version in its User-Agent, which
//! GitHub asks every client to send, and the Accept header its API
//! documents; no token, no query, no compression asked for. It goes
//! over HTTPS only and follows at most two redirects, through a proxy
//! the environment names if there is one. The answer's tag is
//! compared with this build's version, and a newer one is offered in
//! the left pane as a link to its release page, which the browser
//! opens. Nothing is downloaded or installed from here.
//!
//! What the last check found is kept in the settings with when it
//! ran, so a launch within the day asks nothing and a launch offline
//! still knows of a release found before.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::*;

/// The one address asked.
pub const LATEST: &str = "https://api.github.com/repos/jessolmstead/greycard/releases/latest";

/// How long a check's answer stands before the launch asks again.
pub const DAY: u64 = 24 * 60 * 60;

/// The longest the request may take, all of it.
const TIMEOUT: Duration = Duration::from_secs(5);

/// This build's version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// What the settings keep of the check.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Kept {
    /// Whether a launch checks. On by default; the Settings sheet's
    /// switch turns it off, and Check now works either way.
    pub check: bool,
    /// When the last check that reached GitHub ran, seconds since the
    /// epoch; zero for never.
    pub checked_at: u64,
    /// The latest release as that check found it: its tag and its page.
    pub tag: String,
    pub url: String,
    /// A release the left pane was told not to offer again, by tag.
    pub dismissed: String,
}

impl Default for Kept {
    fn default() -> Self {
        Self {
            check: true,
            checked_at: 0,
            tag: String::new(),
            url: String::new(),
            dismissed: String::new(),
        }
    }
}

/// A release as the API names it.
#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub tag: String,
    pub url: String,
}

/// What a check came to.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// GitHub answered with a release.
    Found(Release),
    /// GitHub answered, but not with a release: a rate limit, a 404,
    /// a 5xx, a body that is not the API's. The day counts as checked.
    Refused,
    /// No answer at all: no DNS, no connection, a timeout.
    Unreachable,
}

/// Why a fetch brought no body.
#[derive(Debug, Clone, PartialEq)]
pub enum FetchError {
    /// The server answered, with a status or a body that will not do.
    Answered(String),
    /// Nothing answered.
    Transport(String),
}

/// A version as compared: the three numbers, then whether it is a
/// release, so `0.1.4-rc.1` comes before `0.1.4`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub release: bool,
}

/// `v1.2.3`, `1.2.3`, `1.2` or `v1.2.3-rc.1`: a `-` suffix makes it a
/// pre-release, a `+` suffix (build metadata) is ignored. None for
/// anything else.
pub fn parse_version(s: &str) -> Option<Version> {
    let s = s.trim();
    let s = s.strip_prefix(['v', 'V']).unwrap_or(s);
    let s = s.split('+').next()?;
    let (core, pre) = match s.split_once('-') {
        Some((core, suffix)) => (core, Some(suffix)),
        None => (s, None),
    };
    if pre.is_some_and(str::is_empty) {
        return None;
    }
    let mut parts = core.split('.');
    let mut next = |required: bool| match parts.next() {
        Some(p) if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => p.parse().ok(),
        None if !required => Some(0),
        _ => None,
    };
    let v = Version {
        major: next(true)?,
        minor: next(true)?,
        patch: next(false)?,
        release: pre.is_none(),
    };
    parts.next().is_none().then_some(v)
}

/// Whether `tag` is a strictly later version than `current`. A tag
/// that does not read as a version is never newer, and a pre-release
/// is offered only to a pre-release build: a release build is never
/// pointed at an -rc.
pub fn is_newer(tag: &str, current: &str) -> bool {
    match (parse_version(tag), parse_version(current)) {
        (Some(t), Some(c)) => (t.release || !c.release) && t > c,
        _ => false,
    }
}

/// The tag as the pane says it: "0.1.4", not "v0.1.4".
pub fn shown(tag: &str) -> &str {
    let t = tag.trim();
    t.strip_prefix(['v', 'V']).unwrap_or(t)
}

/// The release in the API's answer: `tag_name` and `html_url`. A page
/// that is not under the repository is not opened: the repository's
/// own releases page stands in for it.
pub fn parse_release(body: &str) -> Option<Release> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let tag = value.get("tag_name")?.as_str()?.trim().to_string();
    if tag.is_empty() {
        return None;
    }
    let url = value
        .get("html_url")
        .and_then(|u| u.as_str())
        .filter(|u| u.starts_with(concat!(env!("CARGO_PKG_REPOSITORY"), "/")))
        .map(str::to_string)
        .unwrap_or_else(|| format!("{}/releases/latest", env!("CARGO_PKG_REPOSITORY")));
    Some(Release { tag, url })
}

/// Seconds since the epoch.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Whether the last check is old enough to ask again: a day gone, or
/// a clock that has gone back past it.
pub fn due(kept: &Kept, now: u64) -> bool {
    kept.checked_at == 0 || now < kept.checked_at || now - kept.checked_at >= DAY
}

/// Whether this launch checks: the switch on, the day gone, and not a
/// batch run (a snapshot, a screenshot, an export), which leaves the
/// network alone as it leaves the settings.
pub fn at_launch(batch: bool, kept: &Kept, now: u64) -> bool {
    !batch && kept.check && due(kept, now)
}

/// The release to offer, from what the settings keep: newer than this
/// build and not dismissed. Its version as shown, and its page.
pub fn offer(kept: &Kept, current: &str) -> Option<(String, String)> {
    (is_newer(&kept.tag, current) && kept.tag != kept.dismissed && !kept.url.is_empty())
        .then(|| (shown(&kept.tag).to_string(), kept.url.clone()))
}

/// The request: a body, or why there is none.
pub type Fetch = fn(&str) -> Result<String, FetchError>;

/// Whether a ureq error came after the server answered.
fn fetch_error(e: ureq::Error) -> FetchError {
    match e {
        ureq::Error::StatusCode(code) => FetchError::Answered(format!("HTTP {code}")),
        e @ ureq::Error::BodyExceedsLimit(_) => FetchError::Answered(e.to_string()),
        e => FetchError::Transport(e.to_string()),
    }
}

/// GET `url` over HTTPS with the User-Agent and Accept headers: no
/// compression asked for, two redirects at most, and whatever proxy
/// the environment names (`HTTPS_PROXY`, `ALL_PROXY` and the like), as
/// ureq does by default.
pub fn github(url: &str) -> Result<String, FetchError> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .https_only(true)
        .max_redirects(2)
        .user_agent(format!("greycard/{VERSION}"))
        .accept("application/vnd.github+json")
        .accept_encoding(ureq::config::AutoHeaderValue::None)
        .build()
        .into();
    let mut response = agent.get(url).call().map_err(fetch_error)?;
    response
        .body_mut()
        .with_config()
        .limit(1 << 20)
        .read_to_string()
        .map_err(fetch_error)
}

/// No network: what a test's editor asks with.
fn offline(_: &str) -> Result<String, FetchError> {
    Err(FetchError::Transport("no network in a test".into()))
}

/// Ask, and read the answer.
pub fn check(fetch: Fetch) -> Outcome {
    match fetch(LATEST) {
        Ok(body) => match parse_release(&body) {
            Some(r) => Outcome::Found(r),
            None => {
                tracing::warn!("update: GitHub's answer names no release");
                Outcome::Refused
            }
        },
        Err(FetchError::Answered(e)) => {
            tracing::warn!("update: GitHub answered {e}");
            Outcome::Refused
        }
        Err(FetchError::Transport(e)) => {
            tracing::info!("update: GitHub not reached: {e}");
            Outcome::Unreachable
        }
    }
}

/// The settings after a check at `now`. A release found is kept with
/// the time. An answer without one (a rate limit, an error page)
/// stamps the time and keeps the release known before, so a GitHub
/// that keeps refusing is still asked once a day, not every launch.
/// A check that reached nothing changes nothing: the next launch
/// tries again, and the release known before still shows. A check
/// `asked` for in the sheet forgets a dismissal of the release it
/// found, since that is what it was asked about.
pub fn after(kept: &Kept, outcome: &Outcome, now: u64, asked: bool) -> Kept {
    match outcome {
        Outcome::Found(r) => Kept {
            checked_at: now,
            tag: r.tag.clone(),
            url: r.url.clone(),
            dismissed: if asked && kept.dismissed == r.tag {
                String::new()
            } else {
                kept.dismissed.clone()
            },
            ..kept.clone()
        },
        Outcome::Refused => Kept {
            checked_at: now,
            ..kept.clone()
        },
        Outcome::Unreachable => kept.clone(),
    }
}

/// The Settings sheet's line for a check asked for there.
pub fn words(outcome: &Outcome, current: &str) -> String {
    match outcome {
        Outcome::Found(r) if is_newer(&r.tag, current) => {
            format!("{} is available", shown(&r.tag))
        }
        Outcome::Found(_) => "You have the latest version".into(),
        Outcome::Refused => "GitHub returned no release; try again later".into(),
        Outcome::Unreachable => "Could not reach GitHub".into(),
    }
}

/// The left pane's entry, from what the settings keep.
pub(crate) fn show(app: &App, kept: &Kept) {
    let (version, url) = offer(kept, VERSION).unwrap_or_default();
    app.set_update_version(version.into());
    app.set_update_url(url.into());
}

thread_local! {
    /// The number of the last Check now, so an answer to an earlier
    /// one is dropped rather than said over the later one's.
    static ASKED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// A check's answer, on the event loop: kept (unless `persist` is
/// off, a batch run's or a test's), the pane's entry shown from it,
/// and for Check now (`asked`, its number) the sheet's line. An answer
/// to a Check now that a later one has overtaken is dropped.
pub(crate) fn apply(app: &App, outcome: &Outcome, now: u64, persist: bool, asked: Option<u64>) {
    if let Some(n) = asked {
        if n != ASKED.with(|a| a.get()) {
            return;
        }
        app.set_update_checking(false);
        app.set_update_note(words(outcome, VERSION).into());
    }
    let kept = if persist {
        let mut settings = settings::Settings::load();
        settings.update = after(&settings.update, outcome, now, asked.is_some());
        settings.save();
        settings.update
    } else if let Outcome::Found(r) = outcome {
        Kept {
            tag: r.tag.clone(),
            url: r.url.clone(),
            ..Kept::default()
        }
    } else {
        // Nothing kept and nothing new: the pane stays as it is.
        return;
    };
    show(app, &kept);
}

/// Run the check off the event loop and bring the answer back to it.
/// The thread is not joined: a quit while it waits leaves it behind.
fn spawn(app: &App, fetch: Fetch, persist: bool, asked: Option<u64>) {
    let app_weak = app.as_weak();
    std::thread::Builder::new()
        .name("update".into())
        .spawn(move || {
            let outcome = check(fetch);
            let now = now();
            let _ = app_weak.upgrade_in_event_loop(move |app| {
                apply(&app, &outcome, now, persist, asked);
            });
        })
        .map(|_| ())
        .unwrap_or_else(|e| tracing::warn!("update: no thread for the check: {e}"));
}

/// At startup: the entry from what was kept, and the check if this
/// launch is due one.
pub(crate) fn start(app: &App, batch: bool, kept: &Kept) {
    app.set_update_check(kept.check);
    show(app, kept);
    if cfg!(test) || !at_launch(batch, kept, now()) {
        return;
    }
    tracing::info!("update: asking GitHub for the latest release");
    spawn(app, github, true, None);
}

/// The switch, Check now, the entry's link and its dismissal.
pub(crate) fn install(app: &App, state: &Rc<RefCell<State>>) {
    let fetch: Fetch = if cfg!(test) { offline } else { github };
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_update_check_changed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let on = app.get_update_check();
            crate::panel::prefs::keep(&state.borrow(), |s| s.update.check = on);
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_update_check_now(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            // One at a time: the button waits for this answer.
            if app.get_update_checking() {
                return;
            }
            let n = ASKED.with(|a| {
                a.set(a.get() + 1);
                a.get()
            });
            app.set_update_checking(true);
            app.set_update_note("Checking...".into());
            let persist = !state.borrow().batch && !cfg!(test);
            spawn(&app, fetch, persist, Some(n));
        });
    }
    {
        let app_weak = app.as_weak();
        app.on_update_open(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let url = app.get_update_url().to_string();
            if url.is_empty() {
                return;
            }
            tracing::info!("update: opening {url}");
            app.set_status(format!("the release page is open in the browser: {url}").into());
            // As the report's form: never from a test.
            if cfg!(test) {
                return;
            }
            std::thread::spawn(move || {
                if let Err(e) = webbrowser::open(&url) {
                    tracing::warn!("update: no browser would open {url}: {e}");
                }
            });
        });
    }
    {
        let (state, app_weak) = (state.clone(), app.as_weak());
        app.on_update_dismissed(move || {
            let Some(app) = app_weak.upgrade() else {
                return;
            };
            let version = app.get_update_version().to_string();
            app.set_update_version("".into());
            app.set_update_url("".into());
            let st = state.borrow();
            crate::panel::prefs::keep(&st, |s| {
                if !s.update.tag.is_empty() && shown(&s.update.tag) == version {
                    s.update.dismissed = s.update.tag.clone();
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_as_numbers() {
        assert!(is_newer("v0.1.3", "0.1.2"));
        assert!(!is_newer("v0.1.2", "0.1.2"));
        assert!(!is_newer("v0.1.2", "0.1.3"));
        assert!(is_newer("0.1.10", "0.1.9"));
        assert!(!is_newer("0.1.9", "0.1.10"));
        assert!(is_newer("v1.0.0", "0.9.9"));
        assert!(is_newer("V0.2", "0.1.9"));
    }

    #[test]
    fn a_pre_release_ranks_below_its_release() {
        // An rc build is offered its final.
        assert!(is_newer("v0.1.4", "0.1.4-rc.1"));
        // Two pre-releases of one version are not told apart: it is
        // the final that an rc build is waiting for.
        assert!(!is_newer("v0.1.4-rc.2", "0.1.4-rc.1"));
        assert!(is_newer("v0.1.5-rc.1", "0.1.4-rc.1"));
        // A release build is never offered an -rc, even a later one.
        assert!(!is_newer("v0.1.4-rc.1", "0.1.3"));
        assert!(!is_newer("v0.1.4-rc.1", "0.1.4"));
        assert!(parse_version("0.1.4-rc.1").unwrap() < parse_version("0.1.4").unwrap());
    }

    #[test]
    fn a_suffix_is_read_and_garbage_is_no_news() {
        let v = |major, minor, patch, release| Version {
            major,
            minor,
            patch,
            release,
        };
        assert_eq!(parse_version("v0.1.4-rc.1"), Some(v(0, 1, 4, false)));
        assert_eq!(parse_version("0.1.4+build.7"), Some(v(0, 1, 4, true)));
        assert_eq!(parse_version("1.2"), Some(v(1, 2, 0, true)));
        assert!(!is_newer("v0.1.3-rc1", "0.1.3"));
        for garbage in [
            "",
            "v",
            "latest",
            "v1",
            "1.x.3",
            "1.2.3.4",
            "-1.2.3",
            "1..2",
            "nightly-2026",
            "1.2.3-",
        ] {
            assert_eq!(parse_version(garbage), None, "{garbage:?}");
            assert!(!is_newer(garbage, "0.1.2"), "{garbage:?}");
        }
        assert!(!is_newer("v9.9.9", "not a version"));
        assert_eq!(parse_version(VERSION).map(|_| ()), Some(()));
    }

    #[test]
    fn the_answer_is_read_for_its_tag_and_page() {
        let body = r#"{
            "url": "https://api.github.com/repos/jessolmstead/greycard/releases/1",
            "html_url": "https://github.com/jessolmstead/greycard/releases/tag/v0.1.4",
            "tag_name": "v0.1.4",
            "name": "greycard 0.1.4",
            "prerelease": false,
            "assets": []
        }"#;
        assert_eq!(
            parse_release(body),
            Some(Release {
                tag: "v0.1.4".into(),
                url: "https://github.com/jessolmstead/greycard/releases/tag/v0.1.4".into(),
            })
        );
        // A page outside the repository is not opened, even on GitHub.
        for elsewhere in [
            "https://example.com/x",
            "https://github.com/someone/else/releases/tag/v0.1.4",
            "http://github.com/jessolmstead/greycard/releases/tag/v0.1.4",
            "https://github.com/jessolmstead/greycard-evil/releases",
        ] {
            let body = format!(r#"{{"tag_name": "v0.1.4", "html_url": "{elsewhere}"}}"#);
            assert_eq!(
                parse_release(&body).unwrap().url,
                format!("{}/releases/latest", env!("CARGO_PKG_REPOSITORY")),
                "{elsewhere}"
            );
        }
        assert_eq!(parse_release(r#"{"message": "Not Found"}"#), None);
        assert_eq!(parse_release(r#"{"tag_name": ""}"#), None);
        assert_eq!(parse_release("<html>rate limited</html>"), None);
    }

    #[test]
    fn a_check_is_its_fetch_read() {
        fn found(url: &str) -> Result<String, FetchError> {
            assert_eq!(url, LATEST);
            Ok(r#"{"tag_name": "v0.1.4", "html_url": "https://github.com/jessolmstead/greycard/releases/tag/v0.1.4"}"#.into())
        }
        fn down(_: &str) -> Result<String, FetchError> {
            Err(FetchError::Transport("dns".into()))
        }
        fn limited(_: &str) -> Result<String, FetchError> {
            Err(FetchError::Answered("HTTP 403".into()))
        }
        fn garbled(_: &str) -> Result<String, FetchError> {
            Ok("<html>unicorn</html>".into())
        }
        let outcome = check(found);
        assert!(matches!(&outcome, Outcome::Found(r) if r.tag == "v0.1.4"));
        assert_eq!(words(&outcome, "0.1.2"), "0.1.4 is available");
        assert_eq!(words(&outcome, "0.1.4"), "You have the latest version");
        assert_eq!(check(down), Outcome::Unreachable);
        assert_eq!(words(&check(down), "0.1.2"), "Could not reach GitHub");
        assert_eq!(check(limited), Outcome::Refused);
        assert_eq!(check(garbled), Outcome::Refused);
    }

    #[test]
    fn a_launch_checks_once_a_day() {
        let t = 1_790_000_000;
        let never = Kept::default();
        assert!(at_launch(false, &never, t));
        let found = Outcome::Found(Release {
            tag: "v0.1.4".into(),
            url: "https://github.com/jessolmstead/greycard/releases/tag/v0.1.4".into(),
        });
        let kept = after(&never, &found, t, false);
        assert_eq!(kept.checked_at, t);
        assert!(!at_launch(false, &kept, t + 60));
        assert!(!at_launch(false, &kept, t + DAY - 1));
        assert!(at_launch(false, &kept, t + DAY));
        // A clock set back is not a reason to wait.
        assert!(at_launch(false, &kept, t - 10));
        // Unreached, nothing changes: the next launch tries again,
        // and what was known stays.
        let again = after(&kept, &Outcome::Unreachable, t + DAY, false);
        assert_eq!(again, kept);
        // The switch off: never at launch.
        let off = Kept {
            check: false,
            ..never.clone()
        };
        assert!(!at_launch(false, &off, t));
    }

    #[test]
    fn a_refusal_counts_as_the_days_check() {
        // A 403 (the rate limit), a 404, a 5xx: GitHub answered, so the
        // next launch within the day does not ask again, and the
        // release known before is kept.
        let t = 1_790_000_000;
        let known = Kept {
            checked_at: t - 2 * DAY,
            tag: "v0.1.4".into(),
            url: "https://github.com/jessolmstead/greycard/releases/tag/v0.1.4".into(),
            ..Kept::default()
        };
        assert!(at_launch(false, &known, t));
        assert_eq!(
            check(|_| Err(FetchError::Answered("HTTP 403".into()))),
            Outcome::Refused
        );
        let refused = after(&known, &Outcome::Refused, t, false);
        assert_eq!(refused.checked_at, t);
        assert_eq!(
            (refused.tag.as_str(), refused.url.as_str()),
            (known.tag.as_str(), known.url.as_str())
        );
        assert!(!at_launch(false, &refused, t + 60));
        assert!(at_launch(false, &refused, t + DAY));
    }

    #[test]
    fn check_now_forgets_a_dismissal_of_what_it_found() {
        let kept = Kept {
            tag: "v0.1.4".into(),
            dismissed: "v0.1.4".into(),
            ..Kept::default()
        };
        let found = Outcome::Found(Release {
            tag: "v0.1.4".into(),
            url: "https://github.com/jessolmstead/greycard/releases/tag/v0.1.4".into(),
        });
        assert_eq!(after(&kept, &found, 1, true).dismissed, "");
        // At launch it stays dismissed.
        assert_eq!(after(&kept, &found, 1, false).dismissed, "v0.1.4");
    }

    #[test]
    fn a_batch_run_never_checks() {
        let t = 1_790_000_000;
        assert!(!at_launch(true, &Kept::default(), t));
        assert!(!at_launch(true, &Kept::default(), t + 10 * DAY));
    }

    #[test]
    fn a_newer_release_is_offered_until_dismissed() {
        let kept = Kept {
            checked_at: 1,
            tag: "v0.1.4".into(),
            url: "https://github.com/jessolmstead/greycard/releases/tag/v0.1.4".into(),
            ..Kept::default()
        };
        assert_eq!(
            offer(&kept, "0.1.2"),
            Some(("0.1.4".into(), kept.url.clone()))
        );
        // Once this build is that version or later, nothing.
        assert_eq!(offer(&kept, "0.1.4"), None);
        assert_eq!(offer(&kept, "0.2.0"), None);
        let dismissed = Kept {
            dismissed: "v0.1.4".into(),
            ..kept.clone()
        };
        assert_eq!(offer(&dismissed, "0.1.2"), None);
        // The next release is offered again.
        let next = Kept {
            tag: "v0.1.5".into(),
            ..dismissed
        };
        assert_eq!(offer(&next, "0.1.2").unwrap().0, "0.1.5");
        assert_eq!(offer(&Kept::default(), "0.1.2"), None);
    }

    #[test]
    fn the_pane_shows_the_kept_release_and_the_sheet_the_check() {
        let app = crate::testing::window(1);
        let _state = crate::testing::state_for(&app, Vec::new());
        let kept = Kept {
            checked_at: 1,
            tag: "v9.9.9".into(),
            url: "https://github.com/jessolmstead/greycard/releases/tag/v9.9.9".into(),
            ..Kept::default()
        };
        start(&app, false, &kept);
        assert!(app.get_update_check(), "on by default");
        assert_eq!(app.get_update_version(), "9.9.9");
        // The link says where it went and opens nothing in a test.
        app.invoke_update_open();
        assert!(app.get_status().contains("releases/tag/v9.9.9"));
        // Not now: gone from the pane.
        app.invoke_update_dismissed();
        assert_eq!(app.get_update_version(), "");
        // Check now: the button waits, and a second press sends nothing.
        app.invoke_update_check_now();
        assert!(app.get_update_checking());
        assert_eq!(app.get_update_note(), "Checking...");
        let first = ASKED.with(|a| a.get());
        app.invoke_update_check_now();
        assert_eq!(ASKED.with(|a| a.get()), first, "one check at a time");
        // Its answer says what it found and frees the button.
        let latest = Outcome::Found(Release {
            tag: format!("v{VERSION}"),
            url: "https://github.com/jessolmstead/greycard/releases/latest".into(),
        });
        apply(&app, &latest, 3, false, Some(first));
        assert!(!app.get_update_checking());
        assert_eq!(app.get_update_note(), "You have the latest version");
        assert_eq!(app.get_update_version(), "");
        // A late answer to an earlier press is dropped.
        app.invoke_update_check_now();
        apply(&app, &Outcome::Unreachable, 4, false, Some(first));
        assert_eq!(app.get_update_note(), "Checking...");
        apply(&app, &Outcome::Unreachable, 5, false, Some(first + 1));
        assert_eq!(app.get_update_note(), "Could not reach GitHub");
    }

    /// The real endpoint, by hand: `cargo test -p greycard-ui
    /// update::tests::the_real_endpoint_by_hand -- --ignored --nocapture`.
    #[test]
    #[ignore = "reaches GitHub; run by hand"]
    fn the_real_endpoint_by_hand() {
        let outcome = check(github);
        println!("GitHub's latest release: {outcome:?}");
        let Outcome::Found(r) = outcome else {
            panic!("GitHub not reached");
        };
        assert!(parse_version(&r.tag).is_some(), "{}", r.tag);
        println!(
            "tag {}, page {}, newer than {VERSION}: {}",
            r.tag,
            r.url,
            is_newer(&r.tag, VERSION)
        );
    }
}
