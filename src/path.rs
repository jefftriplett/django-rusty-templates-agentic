use std::borrow::Cow;
use std::path::PathBuf;

use miette::{Diagnostic, SourceSpan};
use thiserror::Error;

use dtl_lexer::types::At;

#[derive(Error, Debug, Diagnostic, PartialEq, Eq)]
pub enum RelativePathError {
    #[error(
        "The relative path '{template_path}' points outside the file hierarchy that template '{origin}' is in."
    )]
    Outside {
        #[label("relative path")]
        at: SourceSpan,
        origin: PathBuf,
        template_path: String,
    },
    #[error("The relative path '{path}' cannot be evaluated due to an unknown template origin.")]
    UnknownOrigin {
        path: String,
        #[label("here")]
        at: SourceSpan,
    },
}

pub fn construct_relative_path<'a>(
    path: &'a str,
    origin: Option<&'a str>,
    at: At,
) -> Result<Option<Cow<'a, str>>, RelativePathError> {
    let adjacent = path.starts_with("./");
    if !adjacent && !path.starts_with("../") {
        return Ok(None);
    }
    match origin {
        Some(origin) => {
            // Template names use POSIX separators even with a Windows loader.
            let directory = origin.rsplit_once('/').map_or("", |(parent, _)| parent);
            let absolute = directory.starts_with('/');
            let mut parts = Vec::new();
            for part in directory.split('/').chain(path.split('/')) {
                match part {
                    "" | "." => {}
                    ".." if parts.last().is_some_and(|part| *part != "..") => {
                        parts.pop();
                    }
                    ".." if absolute => {}
                    part => parts.push(part),
                }
            }
            if parts.first() == Some(&"..") {
                return Err(RelativePathError::Outside {
                    at: at.into(),
                    origin: origin.into(),
                    template_path: path.to_string(),
                });
            }
            let path = parts.join("/");
            Ok(Some(Cow::Owned(if absolute {
                format!("/{path}")
            } else if path.is_empty() {
                ".".to_string()
            } else {
                path
            })))
        }
        None => Err(RelativePathError::UnknownOrigin {
            at: at.into(),
            path: path.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_names_keep_posix_separators() {
        for (path, origin, expected) in [
            ("./part.html", "nested/page.html", "nested/part.html"),
            ("../part.html", "nested/page.html", "part.html"),
            ("./part.html", "café/page.html", "café/part.html"),
            ("./part.html", "nested\\page.html", "part.html"),
        ] {
            assert_eq!(
                construct_relative_path(path, Some(origin), (0, path.len()))
                    .unwrap()
                    .unwrap(),
                expected
            );
        }
    }

    #[test]
    fn test_construct_adjacent_path_origin_root() {
        let path = "./foo.txt";
        let origin = Some("/");
        let at = (0, 8);
        assert_eq!(
            construct_relative_path(path, origin, at).unwrap().unwrap(),
            "foo.txt"
        );
    }

    #[test]
    fn test_construct_parent_path_origin_root() {
        let path = "../foo.txt";
        let origin = "/";
        let at = (0, 8);
        assert_eq!(
            construct_relative_path(path, Some(origin), at).unwrap_err(),
            RelativePathError::Outside {
                at: at.into(),
                origin: origin.into(),
                template_path: path.to_string(),
            }
        );
    }

    #[test]
    fn test_construct_adjacent_path_origin_empty_string() {
        let path = "./foo.txt";
        let origin = Some("");
        let at = (0, 8);
        assert_eq!(
            construct_relative_path(path, origin, at).unwrap().unwrap(),
            "foo.txt"
        );
    }

    #[test]
    fn test_construct_parent_path_origin_empty_string() {
        let path = "../foo.txt";
        let origin = "";
        let at = (0, 8);
        assert_eq!(
            construct_relative_path(path, Some(origin), at).unwrap_err(),
            RelativePathError::Outside {
                at: at.into(),
                origin: origin.into(),
                template_path: path.to_string(),
            }
        );
    }
}
