//! Walk S3 listing pages to a bound: which page to ask for next, which entries to keep, and where a stopped run goes on.
use crate::domain::types::s3_browse::{S3Entry, S3Object, S3Page, S3Position};

/// A listing in progress. The caller asks [`Scan::next_page`] which page to
/// fetch, hands the page to [`Scan::accept`], and repeats until there is none.
#[derive(Debug)]
pub struct Scan {
    position: S3Position,
    limit: u64,
    scanned: u64,
    complete: bool,
    prefixes: Vec<String>,
    objects: Vec<S3Object>,
}

/// What a finished scan found.
#[derive(Debug, PartialEq, Eq)]
pub struct Scanned {
    pub prefixes: Vec<String>,
    pub objects: Vec<S3Object>,
    pub scanned: u64,
    /// Where the next run goes on; `None` when nothing is left.
    pub resume: Option<S3Position>,
}

impl Scan {
    pub fn new(start: S3Position, limit: u64) -> Self {
        Self {
            position: start,
            limit,
            scanned: 0,
            complete: false,
            prefixes: vec![],
            objects: vec![],
        }
    }

    /// The continuation token of the page to fetch next (`Some(None)` is the
    /// first page), or `None` when the scan is over: nothing is left, or the
    /// bound was reached. A bound reached at the end of a page asks for
    /// nothing more, so no page is fetched only to be put back.
    pub fn next_page(&self) -> Option<Option<String>> {
        (!self.complete && self.scanned < self.limit).then(|| self.position.token.clone())
    }

    /// Examine the page `next_page` asked for, from where the position says,
    /// up to the bound, keeping the entries `keep` accepts.
    pub fn accept(&mut self, page: S3Page, keep: impl Fn(&S3Entry) -> bool) {
        let start = self.position.skip.min(page.entries.len());
        let left = usize::try_from(self.limit - self.scanned).unwrap_or(usize::MAX);
        let end = start + (page.entries.len() - start).min(left);
        for entry in &page.entries[start..end] {
            if keep(entry) {
                match entry {
                    S3Entry::Prefix(prefix) => self.prefixes.push(prefix.clone()),
                    S3Entry::Object(object) => self.objects.push(object.clone()),
                }
            }
        }
        self.scanned += (end - start) as u64;
        if end < page.entries.len() {
            self.position.skip = end;
            return;
        }
        match page.next_token {
            Some(token) => {
                self.position = S3Position {
                    token: Some(token),
                    skip: 0,
                }
            }
            None => self.complete = true,
        }
    }

    /// The objects kept so far, for a caller that shows them as they come.
    pub fn objects(&self) -> &[S3Object] {
        &self.objects
    }

    /// Entries examined so far.
    pub fn scanned(&self) -> u64 {
        self.scanned
    }

    pub fn finish(self) -> Scanned {
        Scanned {
            prefixes: self.prefixes,
            objects: self.objects,
            scanned: self.scanned,
            resume: (!self.complete).then_some(self.position),
        }
    }
}

/// Whether a key contains the text, byte for byte: case matters, nothing is
/// a pattern.
pub fn key_contains(entry: &S3Entry, text: &str) -> bool {
    matches!(entry, S3Entry::Object(object) if object.key.contains(text))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(key: &str) -> S3Entry {
        S3Entry::Object(S3Object {
            key: key.into(),
            size: 1,
            last_modified: None,
            etag: None,
            storage_class: None,
        })
    }

    /// A bucket listed `size` entries a page, the token naming the offset.
    fn pages(keys: &[&str], size: usize) -> impl Fn(Option<String>) -> S3Page {
        let keys: Vec<String> = keys.iter().map(|k| (*k).to_owned()).collect();
        move |token| {
            let start = token.map_or(0, |t| t.parse().unwrap());
            let end = (start + size).min(keys.len());
            S3Page {
                entries: keys[start..end].iter().map(|k| object(k)).collect(),
                next_token: (end < keys.len()).then(|| end.to_string()),
            }
        }
    }

    /// Run scans of `limit` entries, each from where the last stopped, and
    /// return every run's keys and how many pages each fetched.
    fn runs(keys: &[&str], size: usize, limit: u64) -> Vec<(Vec<String>, usize)> {
        let fetch = pages(keys, size);
        let mut start = Some(S3Position::default());
        let mut runs = vec![];
        while let Some(position) = start {
            let mut scan = Scan::new(position, limit);
            let mut fetched = 0;
            while let Some(token) = scan.next_page() {
                fetched += 1;
                assert!(fetched <= keys.len() + 1, "a run keeps fetching pages");
                scan.accept(fetch(token), |_| true);
            }
            let done = scan.finish();
            runs.push((done.objects.into_iter().map(|o| o.key).collect(), fetched));
            start = done.resume;
        }
        runs
    }

    #[test]
    fn s3_scan_resumes_without_skipping_or_repeating_at_page_boundaries() {
        let keys = ["a", "b", "c", "d", "e", "f", "g"];
        for size in 1..=8 {
            for limit in 1..=8 {
                let runs = runs(&keys, size, limit);
                let seen: Vec<String> = runs.iter().flat_map(|(keys, _)| keys.clone()).collect();
                assert_eq!(seen, keys, "page size {size}, limit {limit}");
                assert!(
                    runs.iter().all(|(keys, _)| keys.len() as u64 <= limit),
                    "page size {size}, limit {limit}"
                );
            }
        }
    }

    #[test]
    fn s3_scan_fetches_no_page_past_its_bound() {
        // Four entries two a page, two a run: each run reads exactly one page.
        let runs = runs(&["a", "b", "c", "d"], 2, 2);
        assert_eq!(
            runs,
            [
                (vec!["a".into(), "b".into()], 1),
                (vec!["c".into(), "d".into()], 1)
            ]
        );
    }

    #[test]
    fn s3_scan_reports_what_it_examined_and_where_it_stopped() {
        let fetch = pages(&["x1", "a1", "x2", "b2", "x3"], 2);
        let mut scan = Scan::new(S3Position::default(), 3);
        for _ in 0..10 {
            let Some(token) = scan.next_page() else { break };
            scan.accept(fetch(token), |entry| key_contains(entry, "x"));
        }
        let done = scan.finish();
        assert_eq!(done.scanned, 3);
        assert_eq!(
            done.objects
                .iter()
                .map(|o| o.key.as_str())
                .collect::<Vec<_>>(),
            ["x1", "x2"]
        );
        assert_eq!(
            done.resume,
            Some(S3Position {
                token: Some("2".into()),
                skip: 1
            })
        );
        let mut scan = Scan::new(S3Position::default(), 10);
        for _ in 0..10 {
            let Some(token) = scan.next_page() else { break };
            scan.accept(fetch(token), |entry| key_contains(entry, "missing"));
        }
        let done = scan.finish();
        assert_eq!(
            (done.scanned, done.objects.len(), done.resume),
            (5, 0, None)
        );
    }

    #[test]
    fn s3_key_search_is_a_case_sensitive_literal() {
        assert!(key_contains(
            &object("reports/Invoice[1].pdf"),
            "Invoice[1]"
        ));
        assert!(!key_contains(&object("reports/Invoice.pdf"), "invoice"));
        assert!(!key_contains(&object("reports/a.pdf"), "*.pdf"));
        assert!(!key_contains(
            &S3Entry::Prefix("invoice/".into()),
            "invoice"
        ));
    }
}
