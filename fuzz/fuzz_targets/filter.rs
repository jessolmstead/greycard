//! The filter language, as the filter bar, the command line, and a
//! smart collection type it: a filter that parses prints as text that
//! parses to the same filter, and its SQL binds one parameter to each
//! `?` it holds.
#![no_main]

use greycard_library::Filter;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(filter) = Filter::parse(text) else {
        return;
    };
    let (sql, params) = filter.to_sql();
    assert_eq!(
        sql.matches('?').count(),
        params.len(),
        "the SQL's places and its parameters differ: {sql}"
    );
    let Some(printed) = greycard_fuzz::filter_text(&filter) else {
        return;
    };
    let again = Filter::parse(&printed)
        .unwrap_or_else(|e| panic!("{text:?} printed as {printed:?}, which does not parse: {e}"));
    assert_eq!(again, filter, "{text:?} printed as {printed:?}");
});
