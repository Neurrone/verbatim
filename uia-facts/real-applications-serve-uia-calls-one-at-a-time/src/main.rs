//! Real applications serve UIA calls one at a time
//! (`uia-facts/real-applications-serve-uia-calls-one-at-a-time/readme.md`).
//!
//! - `findall`: the provider program's window with a tree of 20,000 nodes;
//!   a `FindAllBuildCache` of its descendants on one thread while another
//!   thread reads node 1's name back to back. Judged: some reads answer
//!   while the search runs, getting in between its requests; the reads'
//!   median and longest wait and the search's time are printed for trends.

use uia_facts_support::args::{Args, trials};
use uia_facts_support::provider::Provider;
use uia_facts_support::report::{Report, Verdict, relation, runs};
use uia_facts_support::stats::{median, range, summary_ms};
use uia_facts_support::uia::{self, Timed};

use windows::Win32::UI::Accessibility::{TreeScope_Descendants, UIA_NamePropertyId};

/// The search's tree: 20,000 nodes, as Edge's page had 20,000 elements.
const NODES: &str = "20000";

fn main() {
    let args = Args::parse();
    uia::mta();
    let mut report = Report::start("real-applications-serve-uia-calls-one-at-a-time");
    if args.wants("findall") {
        report.case("findall", "a 20,000-node tree searched with a cache request on one thread while another reads a name back to back");
        findall(&mut report);
    }
    report.finish();
}

fn findall(report: &mut Report) {
    let mut between = Vec::new();
    let mut medians = Vec::new();
    let mut longest = Vec::new();
    let mut searches = Vec::new();
    for _ in 0..trials() {
        let provider = Provider::launch(&["--windows", "uia:0", "--nodes", NODES]);
        let client = uia::client();
        uia::set_timeouts(&client, 2_000, 30_000);
        let root = uia::element(&client, provider.hwnd(0)).expect("the window's element");
        let first = uia::share(&uia::node(&client, &root, 1).expect("node 1"));
        let root = uia::share(&root);
        let search = uia::spawn(move || {
            let client = uia::client();
            let request =
                uia::cache_request(&client, &[UIA_NamePropertyId]).expect("a cache request");
            // SAFETY: a live client object.
            let condition = unsafe { client.CreateTrueCondition() }.expect("a condition");
            Timed::of(|| {
                // SAFETY: live element, condition and cache request.
                unsafe {
                    uia::resolve(&root).FindAllBuildCache(
                        TreeScope_Descendants,
                        &condition,
                        &request,
                    )
                }
            })
        });
        let element = uia::resolve(&first);
        let mut reads = Vec::new();
        while !search.is_finished() {
            reads.push(Timed::of(|| uia::name(&element)));
        }
        let search = search.join().expect("the search");
        assert_eq!(search.outcome, "ok", "the search answers");
        between.push(
            reads
                .iter()
                .any(|r| r.outcome == "ok" && r.end < search.end && r.start > search.start),
        );
        let times: Vec<f64> = reads.iter().map(Timed::ms).collect();
        medians.push(median(&times));
        longest.push(range(&times).1);
        searches.push(search.ms());
    }
    report.claim(
        "reads beside the search answer while it runs, between its requests",
        &runs(&between),
        "true",
        relation(&between),
    );
    report.claim(
        "the reads' median",
        &summary_ms(&medians),
        "0.4 ms beside Edge's 20,000-element page",
        Verdict::Trend,
    );
    report.claim(
        "the longest read",
        &summary_ms(&longest),
        "up to 696 ms beside Edge's page",
        Verdict::Trend,
    );
    report.claim("the search", &summary_ms(&searches), "", Verdict::Trend);
}
