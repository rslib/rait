use crate::{ErrorCode, ErrorInfo};
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PageRange {
    All,
    Single(u32),
    Range(u32, u32),
}

impl PageRange {
    pub fn parse(input: Option<&str>) -> Result<Self, ErrorInfo> {
        let Some(input) = input else {
            return Ok(Self::All);
        };
        if let Ok(page) = input.parse::<u32>()
            && page > 0
        {
            return Ok(Self::Single(page));
        }
        if let Some((start, end)) = input.split_once('-') {
            let start = start.parse::<u32>().map_err(|_| invalid_range())?;
            let end = end.parse::<u32>().map_err(|_| invalid_range())?;
            if start > 0 && end >= start {
                return Ok(Self::Range(start, end));
            }
        }
        Err(invalid_range())
    }

    /// First and last item, checked against the item count. `flag` names the
    /// option the range came from, for the error hint.
    pub fn bounds(self, count: u32, flag: &str) -> Result<(u32, u32), ErrorInfo> {
        let (first, last) = match self {
            Self::All => (1, count),
            Self::Single(page) => (page, page),
            Self::Range(first, last) => (first, last),
        };
        if count == 0 || last > count {
            return Err(ErrorInfo::new(
                ErrorCode::InvalidPageRange,
                format!("Range {self} is outside the document, which has {count}."),
                format!("Use {flag} between 1 and {count}."),
            ));
        }
        Ok((first, last))
    }
}

impl fmt::Display for PageRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::All => f.write_str("all"),
            Self::Single(page) => write!(f, "{page}"),
            Self::Range(first, last) => write!(f, "{first}-{last}"),
        }
    }
}

fn invalid_range() -> ErrorInfo {
    ErrorInfo::new(
        ErrorCode::InvalidPageRange,
        "Range must be a number like `4` or a closed range like `2-7`.",
        "Open and comma-separated ranges are not supported yet.".to_string(),
    )
}
