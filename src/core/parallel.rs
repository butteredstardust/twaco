//! Small, bounded parallel operations for independent read-only work.

use super::progress::{Progress, NONE};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Enough workers to hide per-request latency, few enough to be polite to a shared server.
const MAX_WORKERS: usize = 8;

/// Apply `f` to every item with bounded concurrency, preserving input order.
pub(crate) fn map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    map_progress(items, &NONE, f)
}

/// Like [`map`], and report one step to `progress` after each item. The caller starts and
/// finishes the phase. Workers call `progress` from their own threads.
pub(crate) fn map_progress<T: Sync, R: Send>(
    items: &[T],
    progress: &dyn Progress,
    f: impl Fn(&T) -> R + Sync,
) -> Vec<R> {
    let f = |item: &T| {
        let result = f(item);
        progress.advance(1);
        result
    };
    if items.len() <= 1 {
        return items.iter().map(f).collect();
    }

    let workers = items.len().min(MAX_WORKERS);
    // Each worker takes the next unclaimed item, so one slow request delays only itself, not a
    // fixed share of the work. Results carry their index and are put back in input order.
    let next = AtomicUsize::new(0);
    // A new thread starts outside any span. Workers enter the caller's span, so their log
    // events stay under the caller's command. They also share the caller's subscriber, which a
    // test may have set for its own thread only.
    let span = tracing::Span::current();
    let dispatch = tracing::dispatcher::get_default(Clone::clone);
    let done: Vec<(usize, R)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                let (f, next) = (&f, &next);
                let span = span.clone();
                let dispatch = dispatch.clone();
                scope.spawn(move || {
                    tracing::dispatcher::with_default(&dispatch, || {
                        let _entered = span.enter();
                        let mut done = Vec::new();
                        loop {
                            let index = next.fetch_add(1, Ordering::Relaxed);
                            let Some(item) = items.get(index) else {
                                break done;
                            };
                            done.push((index, f(item)));
                        }
                    })
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("parallel worker panicked"))
            .collect()
    });
    let mut slots: Vec<Option<R>> = std::iter::repeat_with(|| None).take(items.len()).collect();
    for (index, result) in done {
        slots[index] = Some(result);
    }
    slots
        .into_iter()
        .map(|slot| slot.expect("every item was claimed by exactly one worker"))
        .collect()
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
    fn a_slow_item_does_not_hold_back_the_items_queued_behind_it() {
        // The first item waits for all the others. With fixed chunks its neighbour would wait
        // behind it in the same worker, and it would time out.
        let items: Vec<usize> = (0..16).collect();
        let finished = AtomicUsize::new(0);
        let result = map(&items, |&item| {
            if item == 0 {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                while finished.load(Ordering::SeqCst) < items.len() - 1 {
                    if std::time::Instant::now() > deadline {
                        return false;
                    }
                    std::thread::yield_now();
                }
            } else {
                finished.fetch_add(1, Ordering::SeqCst);
            }
            true
        });
        assert!(result.iter().all(|&ok| ok));
    }

    #[test]
    fn progress_counts_every_item_from_the_workers() {
        let recorder = crate::core::progress::Recorder::default();
        let items: Vec<usize> = (0..20).collect();
        let result = map_progress(&items, &recorder, |item| item + 1);
        assert_eq!(result.len(), 20);
        assert_eq!(recorder.advanced(), 20);
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
