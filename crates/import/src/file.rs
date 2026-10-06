//! A look at the top of a file: its column titles and a few rows, for the
//! matching step.

/// What the first rows of a file show.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Peek {
    /// One title per column: the first row when it holds titles, else
    /// "Column 1", "Column 2", ... as far as the widest row seen.
    pub titles: Vec<String>,
    /// The first rows after the titles, at most [`SAMPLE_ROWS`].
    pub samples: Vec<Vec<String>>,
}

/// How many rows the matching step shows under each title.
pub(crate) const SAMPLE_ROWS: usize = 3;

/// How much of the file the matching step reads. Enough for the titles and the
/// samples; a row cut off at the end shows as far as it was read.
pub(crate) const PEEK_BYTES: usize = 64 * 1024;

/// Reads the titles and the first rows out of `bytes`, which may end mid-row.
pub(crate) fn peek(bytes: &[u8], delimiter: u8, titles: bool) -> Peek {
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(titles)
        .flexible(true)
        .from_reader(bytes);
    let mut out = Peek::default();
    if titles {
        if let Ok(headers) = reader.headers() {
            out.titles = headers.iter().map(str::to_string).collect();
        }
    }
    for record in reader.records().take(SAMPLE_ROWS).flatten() {
        if record.iter().all(|v| v.trim().is_empty()) {
            continue;
        }
        out.samples
            .push(record.iter().map(str::to_string).collect());
    }
    if !titles {
        let width = out.samples.iter().map(Vec::len).max().unwrap_or(0);
        out.titles = (1..=width).map(|n| format!("Column {n}")).collect();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_come_from_the_first_row() {
        let p = peek(
            b"code,name\nlaunch,Launch\n\nrelaunch,Relaunch\n",
            b',',
            true,
        );
        assert_eq!(p.titles, ["code", "name"]);
        assert_eq!(
            p.samples,
            [vec!["launch", "Launch"], vec!["relaunch", "Relaunch"]]
        );
    }

    #[test]
    fn a_file_without_titles_is_numbered_by_its_widest_row() {
        let p = peek(b"a;b\nc;d;e\n", b';', false);
        assert_eq!(p.titles, ["Column 1", "Column 2", "Column 3"]);
        assert_eq!(p.samples.len(), 2);
    }

    #[test]
    fn a_row_cut_off_at_the_end_still_reads() {
        let p = peek(b"code,name\nlaunch,Launch\nrelaunch,Rel", b',', true);
        assert_eq!(
            p.samples,
            [vec!["launch", "Launch"], vec!["relaunch", "Rel"]]
        );
    }
}
