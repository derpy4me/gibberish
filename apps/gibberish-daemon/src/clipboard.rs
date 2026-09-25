//! Clipboard synchronization, provenance hash tracking, and loopback echo suppression (R14, R15, KTD5).

use arboard::Clipboard;
use gibberish_crypto::secrecy::Secret;
use std::time::{Duration, Instant};

pub const ECHO_SUPPRESSION_WINDOW: Duration = Duration::from_millis(500);

pub struct ClipboardManager {
    clipboard: Option<Clipboard>,
    last_hash: [u8; 32],
    suppress_until: Instant,
    suppressed_hash: [u8; 32],
}

impl Default for ClipboardManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ClipboardManager {
    pub fn new() -> Self {
        Self {
            clipboard: Clipboard::new().ok(),
            last_hash: [0u8; 32],
            suppress_until: Instant::now(),
            suppressed_hash: [0u8; 32],
        }
    }

    /// Read current text from OS clipboard, returning Secret<String> (R14).
    /// Ignores changes that match the provenance hash of recent remote writes within 500ms (R15).
    pub fn read_clipboard(&mut self) -> Option<Secret<String>> {
        let mut content = None;

        #[cfg(target_os = "linux")]
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            // Prioritize native Wayland wl-paste when on Wayland
            if let Ok(output) = std::process::Command::new("wl-paste")
                .arg("--no-newline")
                .output()
            {
                if output.status.success() {
                    if let Ok(text) = String::from_utf8(output.stdout) {
                        if !text.is_empty() {
                            content = Some(text);
                        }
                    }
                }
            }
        }

        if content.is_none() {
            if let Some(ref mut cb) = self.clipboard {
                if let Ok(text) = cb.get_text() {
                    if !text.is_empty() {
                        content = Some(text);
                    }
                }
            }
        }

        #[cfg(target_os = "linux")]
        if content.is_none() {
            // Fallback to native Wayland wl-paste if arboard couldn't capture
            if let Ok(output) = std::process::Command::new("wl-paste")
                .arg("--no-newline")
                .output()
            {
                if output.status.success() {
                    if let Ok(text) = String::from_utf8(output.stdout) {
                        if !text.is_empty() {
                            content = Some(text);
                        }
                    }
                }
            }
        }

        #[cfg(target_os = "macos")]
        if content.is_none() {
            // Fallback to native macOS pbpaste if arboard couldn't capture
            if let Ok(output) = std::process::Command::new("pbpaste").output() {
                if output.status.success() {
                    if let Ok(text) = String::from_utf8(output.stdout) {
                        if !text.is_empty() {
                            content = Some(text);
                        }
                    }
                }
            }
        }

        if let Some(text) = content {
            let hash = blake3::hash(text.as_bytes());
            let hash_bytes = *hash.as_bytes();

            // Provenance echo suppression check (R15, KTD5):
            if Instant::now() < self.suppress_until && hash_bytes == self.suppressed_hash {
                return None;
            }

            if hash_bytes != self.last_hash {
                self.last_hash = hash_bytes;
                return Some(Secret::new(text));
            }
        }
        None
    }

    /// Write received decrypted text into OS clipboard with provenance hash suppression (R15).
    pub fn write_clipboard(&mut self, text: &Secret<String>) -> Result<(), String> {
        let hash = blake3::hash(text.expose_secret().as_bytes());
        let hash_bytes = *hash.as_bytes();

        // Arm loopback echo suppression window (500ms)
        self.suppressed_hash = hash_bytes;
        self.suppress_until = Instant::now() + ECHO_SUPPRESSION_WINDOW;
        self.last_hash = hash_bytes;

        let mut success = false;

        #[cfg(target_os = "linux")]
        {
            // On Linux, always attempt wl-copy first if Wayland session exists.
            // Using child.stdin.take() and explicit drop is mandatory so child receives EOF;
            // otherwise child.wait() hangs indefinitely waiting for EOF.
            use std::io::Write;
            if let Ok(mut child) = std::process::Command::new("wl-copy")
                .stdin(std::process::Stdio::piped())
                .spawn()
            {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(text.expose_secret().as_bytes());
                    drop(stdin);
                }
                if let Ok(status) = child.wait() {
                    if status.success() {
                        success = true;
                    }
                }
            }
        }

        // Also update arboard for X11 / XWayland / cross-desktop clients
        if let Some(ref mut cb) = self.clipboard {
            if cb.set_text(text.expose_secret().clone()).is_ok() {
                success = true;
            }
        }

        #[cfg(target_os = "macos")]
        if !success {
            use std::io::Write;
            if let Ok(mut child) = std::process::Command::new("pbcopy")
                .stdin(std::process::Stdio::piped())
                .spawn()
            {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(text.expose_secret().as_bytes());
                    drop(stdin);
                }
                let _ = child.wait();
                success = true;
            }
        }

        if success {
            Ok(())
        } else {
            Err("Clipboard write failed (no display or clipboard server available)".to_string())
        }
    }

    /// Arm suppression window manually (used in tests or simulation)
    pub fn arm_suppression(&mut self, text: &str) {
        let hash = blake3::hash(text.as_bytes());
        self.suppressed_hash = *hash.as_bytes();
        self.suppress_until = Instant::now() + ECHO_SUPPRESSION_WINDOW;
        self.last_hash = self.suppressed_hash;
    }

    pub fn is_suppressed(&self, text: &str) -> bool {
        let hash = blake3::hash(text.as_bytes());
        Instant::now() < self.suppress_until && *hash.as_bytes() == self.suppressed_hash
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_echo_suppression_window() {
        let mut mgr = ClipboardManager::new();
        let remote_text = "RemoteSecretPayload-42";

        // Arm suppression for remote text
        mgr.arm_suppression(remote_text);

        // Within suppression window: identical text is suppressed
        assert!(mgr.is_suppressed(remote_text));

        // Within suppression window: different user text is NOT suppressed
        assert!(!mgr.is_suppressed("NewLocalUserCopy"));

        // After window expires (simulated with elapsed time)
        mgr.suppress_until = Instant::now() - Duration::from_millis(1);
        assert!(!mgr.is_suppressed(remote_text));
    }
}
