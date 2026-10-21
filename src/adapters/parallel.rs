//! Order-preserving parallel map over std threads (no runtime dependency).

/// Map `f` over `items` on all cores, keeping the input order.
pub fn parallel_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let workers = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(items.len().max(1));
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: Vec<std::sync::Mutex<Option<R>>> = items.iter().map(|_| std::sync::Mutex::new(None)).collect();
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if i >= items.len() {
                        break;
                    }
                    let value = f(&items[i]);
                    *results[i].lock().expect("result slot") = Some(value);
                }
            });
        }
    });
    results
        .into_iter()
        .map(|slot| slot.into_inner().expect("result slot").expect("computed"))
        .collect()
}
