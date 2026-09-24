use rait_core::{ErrorCode, ErrorInfo};
use std::ffi::OsStr;
use std::io::ErrorKind;
use std::process::{Command, Output};

const STDERR_LIMIT: usize = 300;

#[derive(Debug, thiserror::Error)]
pub enum ExecError {
    #[error("`{0}` was not found.")]
    Missing(&'static str),
    #[error("`{program}` could not start: {source}")]
    Spawn {
        program: &'static str,
        source: std::io::Error,
    },
    #[error("`{program}` exited with status {status}: {stderr}")]
    Failed {
        program: &'static str,
        status: i32,
        stderr: String,
    },
}

impl ExecError {
    pub fn status(&self) -> Option<i32> {
        match self {
            Self::Failed { status, .. } => Some(*status),
            _ => None,
        }
    }

    pub fn stderr(&self) -> &str {
        match self {
            Self::Failed { stderr, .. } => stderr,
            _ => "",
        }
    }
}

impl From<ExecError> for ErrorInfo {
    fn from(err: ExecError) -> Self {
        match &err {
            ExecError::Missing(program) => ErrorInfo::new(
                ErrorCode::DependencyMissing,
                err.to_string(),
                format!("Run `rait doctor` to see how to install `{program}`."),
            ),
            ExecError::Spawn { program, .. } | ExecError::Failed { program, .. } => ErrorInfo::new(
                ErrorCode::DependencyFailed,
                format!("`{program}` failed."),
                "Run `rait doctor` to check the install.".to_string(),
            )
            .with_detail(err.to_string()),
        }
    }
}

/// Run `program` without a shell and return its output if it exits with status 0.
pub fn run<I, S>(program: &'static str, args: I) -> Result<Output, ExecError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new(program)
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .map_err(|source| match source.kind() {
            ErrorKind::NotFound => ExecError::Missing(program),
            _ => ExecError::Spawn { program, source },
        })?;
    if output.status.success() {
        return Ok(output);
    }
    Err(ExecError::Failed {
        program,
        status: output.status.code().unwrap_or(-1),
        stderr: sanitize(&output.stderr),
    })
}

fn sanitize(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match joined.char_indices().nth(STDERR_LIMIT) {
        Some((cut, _)) => format!("{}...", &joined[..cut]),
        None => joined,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_program_is_dependency_missing() {
        let err = run("rait-no-such-program", ["x"]).unwrap_err();
        assert!(matches!(err, ExecError::Missing(_)));
        assert_eq!(ErrorInfo::from(err).code, ErrorCode::DependencyMissing);
    }

    #[test]
    fn nonzero_status_is_dependency_failed() {
        let err = run("sh", ["-c", "echo boom >&2; exit 3"]).unwrap_err();
        assert_eq!(err.status(), Some(3));
        assert_eq!(err.stderr(), "boom");
        assert_eq!(ErrorInfo::from(err).code, ErrorCode::DependencyFailed);
    }

    #[test]
    fn stderr_is_bounded() {
        let long = "a ".repeat(1000);
        assert!(sanitize(long.as_bytes()).chars().count() <= STDERR_LIMIT + 3);
    }
}
