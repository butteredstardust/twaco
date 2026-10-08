//! Small, bounded parallel operations for independent read-only work.

/// Enough workers to hide per-request latency, few enough to be polite to a shared server.
const MAX_WORKERS: usize = 8;

/// Apply `f` to every item with bounded concurrency, preserving input order.
pub fn map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    if items.len() <= 1 {
        return items.iter().map(f).collect();
    }

    let workers = items.len().min(MAX_WORKERS);
    let chunk_size = items.len().div_ceil(workers);
    // A new thread starts outside any span. Workers enter the caller's span, so their log
    // events stay under the caller's command. They also share the caller's subscriber, which a
    // test may have set for its own thread only.
    let span = tracing::Span::current();
    let dispatch = tracing::dispatcher::get_default(Clone::clone);
    std::thread::scope(|scope| {
        let handles: Vec<_> = items
            .chunks(chunk_size)
            .map(|chunk| {
                let f = &f;
                let span = span.clone();
                let dispatch = dispatch.clone();
                scope.spawn(move || {
                    tracing::dispatcher::with_default(&dispatch, || {
                        let _entered = span.enter();
                        chunk.iter().map(f).collect::<Vec<_>>()
                    })
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("parallel worker panicked"))
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    #[test]
    fn zero_items_runs_nothing() {
        let calls = AtomicUsize::new(0);
        let items: [usize; 0] = [];
        let result: Vec<usize> = map(&items, |_| {
            calls.fetch_add(1, Ordering::Relaxed);
            0
        });
        assert!(result.is_empty());
        assert_eq!(calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn one_item_runs_once() {
        let calls = AtomicUsize::new(0);
        assert_eq!(
            map(&[7], |item| {
                calls.fetch_add(1, Ordering::Relaxed);
                item * 2
            }),
            [14]
        );
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn many_items_run_once_and_stay_in_order() {
        let items: Vec<usize> = (0..64).collect();
        let calls: Vec<AtomicUsize> = (0..items.len()).map(|_| AtomicUsize::new(0)).collect();
        let threads = Mutex::new(HashSet::new());
        let result = map(&items, |item| {
            calls[*item].fetch_add(1, Ordering::Relaxed);
            threads.lock().unwrap().insert(std::thread::current().id());
            // Make completion order differ from input order.
            std::thread::sleep(std::time::Duration::from_micros((64 - item) as u64));
            item * 3
        });
        assert_eq!(
            result,
            items.iter().map(|item| item * 3).collect::<Vec<_>>()
        );
        assert!(calls.iter().all(|count| count.load(Ordering::Relaxed) == 1));
        assert!(threads.lock().unwrap().len() > 1);
    }

    #[test]
    fn workers_log_under_the_callers_span() {
        let ((), text) = crate::core::diagnostics::captured(|| {
            let span = tracing::info_span!("caller_span");
            let _entered = span.enter();
            map(&[1, 2, 3, 4], |_| tracing::info!("worker event"));
        });
        let lines: Vec<&str> = text
            .lines()
            .filter(|l| l.contains("worker event"))
            .collect();
        assert_eq!(lines.len(), 4, "{text}");
        assert!(lines.iter().all(|l| l.contains("caller_span")), "{text}");
    }
}
