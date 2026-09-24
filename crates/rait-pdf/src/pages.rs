use rait_core::{ErrorCode, ErrorInfo};
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

    /// First and last page, checked against the document page count.
    pub fn bounds(self, page_count: u32) -> Result<(u32, u32), ErrorInfo> {
        let (first, last) = match self {
            Self::All => (1, page_count),
            Self::Single(page) => (page, page),
            Self::Range(first, last) => (first, last),
        };
        if page_count == 0 || last > page_count {
            return Err(ErrorInfo::new(
                ErrorCode::InvalidPageRange,
                format!("Page range {self} is outside the PDF page count {page_count}."),
                format!("Use --pages between 1 and {page_count}."),
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
        "Page range must be a page like `4` or a closed range like `2-7`.",
        "Open and comma-separated ranges are not supported yet.".to_string(),
    )
}
