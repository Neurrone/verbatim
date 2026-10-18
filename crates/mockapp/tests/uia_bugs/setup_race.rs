//! `docs/uia-bugs.md`, "A cache request fails while UIA's first-time setup
//! runs on another thread": several threads of a fresh process, as an
//! outpost starts its UIA threads together, each create a client and build
//! a cache request at once. They all succeed only because `verbatim_uia`
//! finishes UIA's first-time setup once before any thread uses UIA. The
//! runner starts each test in a process of its own, so UIA has not been set
//! up when this one starts.

use verbatim_uia::Shape as _;

pub fn clients_created_at_once_in_a_fresh_process_all_build_cache_requests() {
    const THREADS: usize = 6;
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(THREADS));
    let threads: Vec<_> = (0..THREADS)
        .map(|_| {
            let barrier = std::sync::Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                let uia = verbatim_uia::Uia::new().map_err(|error| format!("client: {error}"))?;
                uia.request::<verbatim_uia::Node>(verbatim_uia::Node::all_on())
                    .map(|_| ())
                    .map_err(|error| format!("cache request: {error}"))
            })
        })
        .collect();
    for thread in threads {
        assert_eq!(thread.join().expect("the thread finishes"), Ok(()));
    }
}
