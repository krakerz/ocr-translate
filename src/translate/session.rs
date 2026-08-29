use std::time::{SystemTime, UNIX_EPOCH};

/// Tracks how many translations a cached scraped-token/cookie session has
/// served. A session is refreshed the first time it's used, and again every
/// random(75, 125) translations after that — matching the reset cadence
/// XUnity.AutoTranslator uses for its free Google/Bing/DeepL endpoints
/// (`XUnity.AutoTranslator/src/Translators/*`), so a session (which costs an
/// extra page-load round-trip to set up: scraping a token or priming
/// cookies) isn't re-fetched on every single translation, but also doesn't
/// go stale forever if the real site invalidates it after a while.
pub struct SessionCounter {
    count: u32,
    reset_after: u32,
}

impl Default for SessionCounter {
    fn default() -> Self {
        Self {
            count: 0,
            reset_after: random_between(75, 125),
        }
    }
}

impl SessionCounter {
    /// Call once per translation attempt, before using the session. Returns
    /// `true` if the session should be (re)established first.
    pub fn tick(&mut self) -> bool {
        let needs_refresh = self.count == 0 || self.count >= self.reset_after;
        self.count += 1;
        if needs_refresh {
            self.count = 1;
            self.reset_after = random_between(75, 125);
        }
        needs_refresh
    }
}

/// A small pseudo-random spread, not a security-sensitive one — this only
/// staggers session-refresh timing so a long-running batch of translations
/// doesn't re-fetch its token/cookies on a perfectly regular schedule.
fn random_between(min: u32, max: u32) -> u32 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    min + nanos % (max - min + 1)
}
