//! Path helpers vendored from Zed's `path` + `util::paths` crates.
//!
//! Only what the terminal fork uses: [`PathStyle`], [`UrlExt`], and
//! [`PathWithPosition`]. Crossh keeps no Zed dependency besides
//! `gpui`/`gpui_platform`.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PathStyle {
    Unix,
    Windows,
}

impl PathStyle {
    #[cfg(target_os = "windows")]
    pub const fn local() -> Self {
        PathStyle::Windows
    }

    #[cfg(not(target_os = "windows"))]
    pub const fn local() -> Self {
        PathStyle::Unix
    }

    pub fn is_absolute(&self, path_like: &str) -> bool {
        path_like.starts_with('/')
            || *self == PathStyle::Windows
                && (path_like.starts_with('\\')
                    || path_like
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_ascii_alphabetic())
                        && path_like[1..]
                            .strip_prefix(':')
                            .is_some_and(|path| path.starts_with('/') || path.starts_with('\\')))
    }

    pub fn is_windows(&self) -> bool {
        *self == PathStyle::Windows
    }

    pub fn is_posix(&self) -> bool {
        *self == PathStyle::Unix
    }
}

/// Why [`UrlExt::to_file_path_ext`] rejected a URL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileUrlError {
    /// Not a `file:` URL, or carries a host this platform style can't use.
    Unsupported,
    /// A path segment isn't valid unicode after percent-decoding.
    InvalidEncoding,
    /// A Windows path that doesn't start with a drive letter or UNC host.
    InvalidWindowsPath,
    /// The URL is too long to decode into a path.
    TooLong,
}

impl std::fmt::Display for FileUrlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported => write!(f, "not a file URL usable on this platform"),
            Self::InvalidEncoding => write!(f, "file URL path is not valid unicode"),
            Self::InvalidWindowsPath => write!(f, "file URL is not a valid Windows path"),
            Self::TooLong => write!(f, "file URL is too long to decode into a path"),
        }
    }
}

impl std::error::Error for FileUrlError {}

pub trait UrlExt {
    /// A version of `url::Url::to_file_path` that does platform handling based
    /// on the provided `PathStyle` instead of the host platform.
    fn to_file_path_ext(&self, path_style: PathStyle) -> Result<PathBuf, FileUrlError>;
}

impl UrlExt for url::Url {
    // Copied from `url::Url::to_file_path`, but the `cfg` handling is replaced with runtime branching on `PathStyle`
    fn to_file_path_ext(&self, source_path_style: PathStyle) -> Result<PathBuf, FileUrlError> {
        if let Some(segments) = self.path_segments() {
            let host = match self.host() {
                None | Some(url::Host::Domain("localhost")) => None,
                Some(_) if source_path_style.is_windows() && self.scheme() == "file" => {
                    self.host_str()
                }
                _ => return Err(FileUrlError::Unsupported),
            };

            let str_len = self.as_str().len();
            let estimated_capacity = if source_path_style.is_windows() {
                // remove scheme: - has possible \\ for hostname
                str_len.saturating_sub(self.scheme().len() + 1)
            } else {
                // remove scheme://
                str_len.saturating_sub(self.scheme().len() + 3)
            };
            return match source_path_style {
                PathStyle::Unix => {
                    file_url_segments_to_pathbuf_posix(estimated_capacity, host, segments)
                }
                PathStyle::Windows => {
                    file_url_segments_to_pathbuf_windows(estimated_capacity, host, segments)
                }
            };
        }

        fn file_url_segments_to_pathbuf_posix(
            estimated_capacity: usize,
            host: Option<&str>,
            segments: std::str::Split<'_, char>,
        ) -> Result<PathBuf, FileUrlError> {
            use percent_encoding::percent_decode;

            if host.is_some() {
                return Err(FileUrlError::Unsupported);
            }

            let mut bytes = Vec::new();
            bytes
                .try_reserve(estimated_capacity)
                .map_err(|_| FileUrlError::TooLong)?;

            for segment in segments {
                bytes.push(b'/');
                bytes.extend(percent_decode(segment.as_bytes()));
            }

            // A windows drive letter must end with a slash.
            if bytes.len() > 2
                && bytes[bytes.len() - 2].is_ascii_alphabetic()
                && matches!(bytes[bytes.len() - 1], b':' | b'|')
            {
                bytes.push(b'/');
            }

            let path = String::from_utf8(bytes).map_err(|_| FileUrlError::InvalidEncoding)?;
            debug_assert!(
                PathStyle::Unix.is_absolute(&path),
                "to_file_path() failed to produce an absolute Path"
            );

            Ok(PathBuf::from(path))
        }

        fn file_url_segments_to_pathbuf_windows(
            estimated_capacity: usize,
            host: Option<&str>,
            mut segments: std::str::Split<'_, char>,
        ) -> Result<PathBuf, FileUrlError> {
            use percent_encoding::percent_decode_str;
            let mut string = String::new();
            string
                .try_reserve(estimated_capacity)
                .map_err(|_| FileUrlError::TooLong)?;
            if let Some(host) = host {
                string.push_str(r"\\");
                string.push_str(host);
            } else {
                let first = segments.next().ok_or(FileUrlError::InvalidWindowsPath)?;

                match first.len() {
                    2 => {
                        if !first.starts_with(|c: char| c.is_ascii_alphabetic())
                            || first.as_bytes()[1] != b':'
                        {
                            return Err(FileUrlError::InvalidWindowsPath);
                        }

                        string.push_str(first);
                    }

                    4 => {
                        if !first.starts_with(|c: char| c.is_ascii_alphabetic()) {
                            return Err(FileUrlError::InvalidWindowsPath);
                        }
                        let bytes = first.as_bytes();
                        if bytes[1] != b'%'
                            || bytes[2] != b'3'
                            || (bytes[3] != b'a' && bytes[3] != b'A')
                        {
                            return Err(FileUrlError::InvalidWindowsPath);
                        }

                        string.push_str(&first[0..1]);
                        string.push(':');
                    }

                    _ => return Err(FileUrlError::InvalidWindowsPath),
                }
            };

            for segment in segments {
                string.push('\\');

                // Currently non-unicode windows paths cannot be represented
                match percent_decode_str(segment).decode_utf8() {
                    Ok(s) => string.push_str(&s),
                    Err(..) => return Err(FileUrlError::InvalidEncoding),
                }
            }
            debug_assert!(
                PathStyle::Windows.is_absolute(&string),
                "to_file_path() failed to produce an absolute Path"
            );
            let path = PathBuf::from(string);
            Ok(path)
        }
        Err(FileUrlError::Unsupported)
    }
}

/// A representation of a path-like string with optional row and column numbers.
/// Matching values example: `te`, `test.rs:22`, `te:22:5`, `test.c(22)`, `test.c(22,5)`etc.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct PathWithPosition {
    pub path: PathBuf,
    pub row: Option<u32>,
    // Absent if row is absent.
    pub column: Option<u32>,
}

/// A delimiter to use in `path_query:row_number:column_number` strings parsing.
pub const FILE_ROW_COLUMN_DELIMITER: char = ':';

const ROW_COL_CAPTURE_REGEX: &str = r"(?xs)
    ([^\(]+)\:(?:
        \((\d+)[,:](\d+)\) # filename:(row,column), filename:(row:column)
        |
        \((\d+)\)()     # filename:(row)
    )
    |
    ([^\(]+)(?:
        \((\d+)[,:](\d+)\) # filename(row,column), filename(row:column)
        |
        \((\d+)\)()     # filename(row)
    )
    \:*$
    |
    (.+?)(?:
        \:+(\d+)\:(\d+)\:*$  # filename:row:column
        |
        \:+(\d+)\:*()$       # filename:row
        |
        \:+()()$
    )";

impl PathWithPosition {
    /// Returns a PathWithPosition from a path.
    pub fn from_path(path: PathBuf) -> Self {
        Self {
            path,
            row: None,
            column: None,
        }
    }

    /// Parses a string that possibly has `:row:column` or `(row, column)` suffix.
    /// Parenthesis format is used by [MSBuild](https://learn.microsoft.com/en-us/visualstudio/msbuild/msbuild-diagnostic-format-for-tasks) compatible tools
    /// Ignores trailing `:`s, so `test.rs:22:` is parsed as `test.rs:22`.
    /// If the suffix parsing fails, the whole string is parsed as a path.
    ///
    /// Be mindful that `test_file:10:1:` is a valid posix filename.
    /// `PathWithPosition` class assumes that the ending position-like suffix is **not** part of the filename.
    pub fn parse_str(s: &str) -> Self {
        let trimmed = s.trim();
        let path = Path::new(trimmed);
        let Some(maybe_file_name_with_row_col) = path.file_name().unwrap_or_default().to_str()
        else {
            return Self {
                path: Path::new(s).to_path_buf(),
                row: None,
                column: None,
            };
        };
        if maybe_file_name_with_row_col.is_empty() {
            return Self {
                path: Path::new(s).to_path_buf(),
                row: None,
                column: None,
            };
        }

        // Let's avoid repeated init cost on this. It is subject to thread contention, but
        // so far this code isn't called from multiple hot paths. Getting contention here
        // in the future seems unlikely.
        static SUFFIX_RE: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(ROW_COL_CAPTURE_REGEX).unwrap());
        match SUFFIX_RE
            .captures(maybe_file_name_with_row_col)
            .map(|caps| caps.extract())
        {
            Some((_, [file_name, maybe_row, maybe_column])) => {
                let row = maybe_row.parse::<u32>().ok();
                let column = maybe_column.parse::<u32>().ok();

                let (_, suffix) = trimmed.split_once(file_name).unwrap();
                let path_without_suffix = &trimmed[..trimmed.len() - suffix.len()];

                Self {
                    path: Path::new(path_without_suffix).to_path_buf(),
                    row,
                    column,
                }
            }
            None => {
                // The `ROW_COL_CAPTURE_REGEX` deals with separated digits only,
                // but in reality there could be `foo/bar.py:22:in` inputs which we want to match too.
                // The regex mentioned is not very extendable with "digit or random string" checks, so do this here instead.
                let delimiter = ':';
                let mut path_parts = s
                    .rsplitn(3, delimiter)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .fuse();
                let mut path_string = path_parts.next().expect("rsplitn should have the rest of the string as its last parameter that we reversed").to_owned();
                let mut row = None;
                let mut column = None;
                if let Some(maybe_row) = path_parts.next() {
                    if let Ok(parsed_row) = maybe_row.parse::<u32>() {
                        row = Some(parsed_row);
                        if let Some(parsed_column) = path_parts
                            .next()
                            .and_then(|maybe_col| maybe_col.parse::<u32>().ok())
                        {
                            column = Some(parsed_column);
                        }
                    } else {
                        path_string.push(delimiter);
                        path_string.push_str(maybe_row);
                    }
                }
                for split in path_parts {
                    path_string.push(delimiter);
                    path_string.push_str(split);
                }

                Self {
                    path: PathBuf::from(path_string),
                    row,
                    column,
                }
            }
        }
    }
}
