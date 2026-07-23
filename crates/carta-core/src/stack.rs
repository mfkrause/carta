//! Running recursion-heavy work on a large, dedicated stack.
//!
//! Some conversions recurse as deeply as their input nests (walking a document tree, decoding a
//! deeply nested markup fragment), so a legitimately deep input can exhaust a small caller stack.
//! [`on_deep_stack`] runs such work on a worker thread that reserves a generous stack for it.

/// Virtual stack reserved for work whose recursion depth tracks input nesting. Large enough that
/// even adversarially deep input cannot exhaust it; on demand-paged systems the reservation stays
/// uncommitted until touched, so the cost is address space rather than resident memory.
pub const DEEP_STACK: usize = 256 * 1024 * 1024;

/// The outcome of running work on a dedicated large stack.
#[derive(Debug)]
pub enum DeepStack<T> {
    /// The work ran to completion, yielding its value.
    Completed(T),
    /// A worker thread started but the work panicked while running on it.
    Panicked,
    /// No worker thread could be spawned, so the work never ran.
    NotSpawned,
}

/// Runs `work` on a dedicated thread with a large reserved stack ([`DEEP_STACK`]), so deeply nested
/// input cannot overflow the caller's stack.
///
/// The closure is consumed whether or not a worker starts, so a caller that wants to retry on the
/// current stack expresses the work as a re-callable expression and rebuilds it in the
/// [`DeepStack::NotSpawned`] arm.
pub fn on_deep_stack<T, F>(work: F) -> DeepStack<T>
where
    T: Send,
    F: FnOnce() -> T + Send,
{
    let joined = std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(DEEP_STACK)
            .spawn_scoped(scope, work)
            .map(std::thread::ScopedJoinHandle::join)
    });
    match joined {
        Ok(Ok(value)) => DeepStack::Completed(value),
        Ok(Err(_)) => DeepStack::Panicked,
        Err(_) => DeepStack::NotSpawned,
    }
}

#[cfg(test)]
mod tests {
    use super::{DeepStack, on_deep_stack};

    #[test]
    fn completed_work_returns_its_value() {
        match on_deep_stack(|| 2 + 3) {
            DeepStack::Completed(value) => assert_eq!(value, 5),
            _ => panic!("work should complete"),
        }
    }

    #[test]
    fn deep_recursion_does_not_overflow_a_small_caller_stack() {
        fn descend(depth: usize) -> usize {
            if depth == 0 {
                0
            } else {
                1 + descend(depth - 1)
            }
        }
        let outcome = std::thread::Builder::new()
            .stack_size(64 * 1024)
            .spawn(|| match on_deep_stack(|| descend(200_000)) {
                DeepStack::Completed(value) => value,
                _ => 0,
            })
            .expect("spawn shallow caller")
            .join()
            .expect("shallow caller finished");
        assert_eq!(outcome, 200_000);
    }
}
