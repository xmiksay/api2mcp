//! `run.rs`'s unit tests, split out to keep that file under the workspace's 400-line cap — same
//! `#[path = "..._tests.rs"]` split as `cli::call`/`http::bind`. Everything requiring a real
//! Postgres (batching, the actual `DELETE`, session purge) lives in `tests/retention.rs`
//! instead: `cutoff_for` is the one piece of this file's logic that's pure.

use super::*;

#[test]
fn zero_retention_days_means_keep_forever() {
    assert!(cutoff_for(0).is_none());
}

#[test]
fn nonzero_retention_days_cuts_off_that_many_days_ago() {
    let before = Utc::now() - Duration::days(30);
    let cutoff = cutoff_for(30).expect("nonzero retention has a cutoff");
    let after = Utc::now() - Duration::days(30);
    // `Utc::now()` moves between the two bracketing calls above, so assert a window rather
    // than an exact instant.
    assert!(cutoff >= before && cutoff <= after);
}
