//! Plain issue data shared by the issue engine and command layer.

use std::fmt;

/// Generic output of every issue source.
#[derive(Debug, Clone)]
pub struct Issue {
    /// Canonical URL of the issue, e.g. "https://github.com/owner/repo/issues/84".
    pub source_id: String,
    pub title: String,
    /// Empty string if the issue has no description.
    pub body: String,
    /// Display name from the issue source provider.
    pub provider: String,
}

impl Issue {
    /// Parses the last path segment of `source_id` as a u32, if possible.
    /// Returns `Some(84)` for ".../issues/84", `None` for ".../PROJ-123".
    pub fn numeric_id(&self) -> Option<u32> {
        self.source_id
            .rsplit('/')
            .next()
            .and_then(|s| s.parse::<u32>().ok())
    }
}

/// Errors from issue source operations.
#[derive(Debug)]
pub enum IssueSourceError {
    NotFound {
        provider: String,
        source_id: String,
    },
    Unauthorized {
        provider: String,
        /// Provider-supplied hint for resolving the auth issue. Empty if no
        /// hint applies. Providers populate this field.
        hint: String,
    },
    RateLimited {
        provider: String,
    },
    InvalidRef {
        provider: String,
        input: String,
        hint: String,
    },
    NoRemoteDetected {
        provider: String,
    },
    NoMatchingProvider {
        input: String,
    },
    Network {
        provider: String,
        detail: String,
    },
    ProviderError {
        provider: String,
        detail: String,
    },
}

impl fmt::Display for IssueSourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound {
                provider,
                source_id,
            } => {
                write!(f, "{provider}: issue not found: {source_id}")
            }
            Self::Unauthorized { provider, hint } => {
                if hint.is_empty() {
                    write!(f, "{provider}: unauthorized")
                } else {
                    write!(f, "{provider}: unauthorized — {hint}")
                }
            }
            Self::RateLimited { provider } => write!(f, "{provider}: API rate limit exceeded"),
            Self::InvalidRef {
                provider,
                input,
                hint,
            } => write!(f, "{provider}: invalid issue reference '{input}': {hint}"),
            Self::NoRemoteDetected { provider } => {
                write!(
                    f,
                    "{provider}: no {provider} remote detected for this repository"
                )
            }
            Self::NoMatchingProvider { input } => {
                write!(f, "no issue provider can handle '{input}'")
            }
            Self::Network { provider, detail } => write!(f, "{provider}: network error: {detail}"),
            Self::ProviderError { provider, detail } => write!(f, "{provider}: {detail}"),
        }
    }
}

impl std::error::Error for IssueSourceError {}

/// Carries the `--issue` flag value. Composed into command flag structs.
#[derive(Debug, Clone, Default)]
pub struct IssueSourceFlags {
    pub issue: Option<String>,
}

/// Converts arbitrary text to a hyphen-delimited, lowercase slug safe for
/// use in filenames and git branch names.
pub fn slugify(text: &str, max_len: usize) -> String {
    let mut out = String::new();
    let mut last_dash = true;
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.len() <= max_len {
        return trimmed.to_string();
    }
    let cut = &trimmed[..max_len];
    if let Some(last_hyphen) = cut.rfind('-') {
        cut[..last_hyphen].to_string()
    } else {
        cut.trim_end_matches('-').to_string()
    }
}
