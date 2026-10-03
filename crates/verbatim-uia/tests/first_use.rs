//! UIA's first-time setup in a process, raced by several threads, as an
//! outpost starts its UIA threads together. A file of its own, so the test
//! runs in a fresh process where UIA has not been set up yet.

#[test]
fn clients_created_at_once_in_a_fresh_process_all_build_cache_requests() {
    const THREADS: usize = 6;
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(THREADS));
    let threads: Vec<_> = (0..THREADS)
        .map(|_| {
            let barrier = std::sync::Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                let uia = verbatim_uia::Uia::new().map_err(|error| format!("client: {error}"))?;
                uia.base_cache_request()
                    .map(|_| ())
                    .map_err(|error| format!("cache request: {error}"))
            })
        })
        .collect();
    for thread in threads {
        assert_eq!(thread.join().expect("the thread finishes"), Ok(()));
    }
}
