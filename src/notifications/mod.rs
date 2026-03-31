//! Desktop notification service.
//!
//! Abstracts the OS-level notification mechanisms to provide a clean, testable
//! interface for notifying users about important events like DMs or mentions.

use crate::config::NotificationConfig;
use notify_rust::{Notification, Timeout};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

// Ensure we don't spam the user with too many notifications too quickly
static LAST_NOTIFICATION_TIME: OnceLock<Mutex<Instant>> = OnceLock::new();

/// The core notification service.
pub struct NotificationService;

impl NotificationService {
    fn rate_limit_clock() -> &'static Mutex<Instant> {
        LAST_NOTIFICATION_TIME.get_or_init(|| Mutex::new(Instant::now() - Duration::from_secs(10)))
    }

    /// Initializes the notification service state.
    pub fn init() {
        if let Ok(mut last_time) = Self::rate_limit_clock().lock() {
            *last_time = Instant::now() - Duration::from_secs(10);
        }
    }

    /// Sends a desktop notification if conditions are met.
    ///
    /// # Arguments
    ///
    /// * `config` - The user's notification preferences.
    /// * `title` - The title of the notification (e.g., "Mention from bob").
    /// * `message` - The body of the notification.
    /// * `force` - If true, bypasses the rate limiter (use sparingly).
    pub fn send(config: &NotificationConfig, title: &str, message: &str, force: bool) {
        if !config.enabled {
            return;
        }

        if !force && Self::is_rate_limited() {
            return;
        }

        let mut notification = Notification::new();
        notification
            .appname("Chatify")
            .summary(title)
            .body(message)
            .timeout(Timeout::Milliseconds(5000));

        // Use thread-spawning to avoid blocking the async reactor or main thread
        let n: notify_rust::Notification = notification.clone();
        std::thread::spawn(move || {
            if let Err(e) = n.show() {
                log::debug!("Failed to send desktop notification: {}", e);
            }
        });
    }

    /// Checks if we should drop a notification to avoid spamming the user.
    /// Rate limit is currently 1 notification every 2 seconds.
    fn is_rate_limited() -> bool {
        if let Ok(mut last_time) = Self::rate_limit_clock().lock() {
            let now = Instant::now();
            if now.duration_since(*last_time) < Duration::from_secs(2) {
                return true;
            }
            *last_time = now;
            return false;
        }
        false
    }
}
