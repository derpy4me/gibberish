//! Type-level secret wrapping and zeroization guardrails (R4, R30).

use core::fmt;
use core::ops::{Deref, DerefMut};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Secret buffer wrapper ensuring sensitive keys, nonces, and plaintexts are zeroized on drop
/// and never leaked through fmt::Debug / fmt::Display.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret<T: Zeroize> {
    inner: T,
}

impl<T: Zeroize> Secret<T> {
    pub fn new(val: T) -> Self {
        Self { inner: val }
    }

    /// Explicitly expose the underlying secret for cryptographic operations.
    pub fn expose_secret(&self) -> &T {
        &self.inner
    }

    /// Explicitly expose mutable underlying secret.
    pub fn expose_secret_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}

impl<T: Zeroize> Drop for Secret<T> {
    fn drop(&mut self) {
        self.inner.zeroize();
    }
}

impl<T: Zeroize> ZeroizeOnDrop for Secret<T> {}

impl<T: Zeroize> Deref for Secret<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T: Zeroize> DerefMut for Secret<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl<T: Zeroize> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<REDACTED>")
    }
}

impl<T: Zeroize> fmt::Display for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<REDACTED>")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_secret_redaction_and_zeroize() {
        let secret = Secret::new([42u8; 32]);
        let debug_str = format!("{:?}", secret);
        assert_eq!(debug_str, "<REDACTED>");

        let display_str = format!("{}", secret);
        assert_eq!(display_str, "<REDACTED>");

        assert_eq!(*secret.expose_secret(), [42u8; 32]);
    }
}
