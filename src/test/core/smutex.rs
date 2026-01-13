#[cfg(test)]
mod tests_smutex {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::thread;
    use crate::core::smutex::SMutex;
    // ==================== Basic Exclusive Lock ====================

    #[test]
    fn test_lock_unlock_basic() {
        let mutex = SMutex::new();
        assert!(!mutex.is_locked());

        let guard = mutex.lock();
        assert!(mutex.is_locked());

        drop(guard);
        assert!(!mutex.is_locked());
    }

    #[test]
    fn test_lock_protects_data() {
        let mutex = Arc::new(SMutex::new());
        let counter = Arc::new(AtomicUsize::new(0));
        const N: usize = 10;
        const ITERS: usize = 100;

        let handles: Vec<_> = (0..N)
            .map(|_| {
                let m = Arc::clone(&mutex);
                let cnt = Arc::clone(&counter);
                thread::spawn(move || {
                    for _ in 0..ITERS {
                        let _guard = m.lock();
                        let val = cnt.load(Ordering::Relaxed);
                        thread::yield_now();
                        cnt.store(val + 1, Ordering::Relaxed);
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(counter.load(Ordering::Relaxed), N * ITERS);
    }

    #[test]
    fn test_mutual_exclusion() {
        let mutex = Arc::new(SMutex::new());
        let in_critical = Arc::new(AtomicUsize::new(0));
        let max_concurrent = Arc::new(AtomicUsize::new(0));
        const N: usize = 5;

        let handles: Vec<_> = (0..N)
            .map(|_| {
                let m = Arc::clone(&mutex);
                let ic = Arc::clone(&in_critical);
                let mc = Arc::clone(&max_concurrent);
                thread::spawn(move || {
                    for _ in 0..10 {
                        let _guard = m.lock();
                        let current = ic.fetch_add(1, Ordering::SeqCst) + 1;
                        mc.fetch_max(current, Ordering::SeqCst);
                        thread::yield_now();
                        ic.fetch_sub(1, Ordering::SeqCst);
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(max_concurrent.load(Ordering::SeqCst), 1, "Only one thread should be in critical section");
    }

    // ==================== Recursive Lock ====================

    #[test]
    fn test_recursive_lock_same_thread() {
        let mutex = SMutex::new();

        let guard1 = mutex.lock();
        assert!(mutex.is_locked());

        let guard2 = mutex.lock(); // Should not deadlock
        assert!(mutex.is_locked());

        let guard3 = mutex.lock(); // Triple recursion
        assert!(mutex.is_locked());

        drop(guard3);
        assert!(mutex.is_locked()); // Still locked (guard1, guard2)

        drop(guard2);
        assert!(mutex.is_locked()); // Still locked (guard1)

        drop(guard1);
        assert!(!mutex.is_locked()); // Now unlocked
    }

    #[test]
    fn test_recursive_lock_count() {
        let mutex = SMutex::new();

        let g1 = mutex.lock();
        assert_eq!(mutex.recursion.load(Ordering::Relaxed), 1);

        let g2 = mutex.lock();
        assert_eq!(mutex.recursion.load(Ordering::Relaxed), 2);

        let g3 = mutex.lock();
        assert_eq!(mutex.recursion.load(Ordering::Relaxed), 3);

        drop(g3);
        assert_eq!(mutex.recursion.load(Ordering::Relaxed), 2);

        drop(g2);
        assert_eq!(mutex.recursion.load(Ordering::Relaxed), 1);

        drop(g1);
        assert_eq!(mutex.recursion.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_deep_recursion() {
        let mutex = SMutex::new();
        const DEPTH: usize = 100;

        fn recurse(m: &SMutex, depth: usize) {
            if depth == 0 {
                return;
            }
            let _guard = m.lock();
            assert!(m.is_locked());
            recurse(m, depth - 1);
        }

        recurse(&mutex, DEPTH);
        assert!(!mutex.is_locked());
    }

    // ==================== Group (Shared) Lock ====================

    #[test]
    fn test_group_lock_basic() {
        let mutex = SMutex::new();

        let guard = mutex.lock_group();
        assert!(!mutex.is_locked()); // GROUP_FLAG doesn't set LOCKED bit

        drop(guard);
    }

    #[test]
    fn test_multiple_group_locks_concurrent() {
        let mutex = Arc::new(SMutex::new());
        let in_group = Arc::new(AtomicUsize::new(0));
        let max_concurrent = Arc::new(AtomicUsize::new(0));
        const N: usize = 5;

        let ready = Arc::new(AtomicUsize::new(0));
        let go = Arc::new(AtomicBool::new(false));

        let handles: Vec<_> = (0..N)
            .map(|_| {
                let m = Arc::clone(&mutex);
                let ig = Arc::clone(&in_group);
                let mc = Arc::clone(&max_concurrent);
                let r = Arc::clone(&ready);
                let g = Arc::clone(&go);
                thread::spawn(move || {
                    r.fetch_add(1, Ordering::AcqRel);
                    while !g.load(Ordering::Acquire) {
                        thread::yield_now();
                    }

                    let _guard = m.lock_group();
                    let current = ig.fetch_add(1, Ordering::SeqCst) + 1;
                    mc.fetch_max(current, Ordering::SeqCst);

                    // Hold the lock for a bit
                    for _ in 0..50 {
                        thread::yield_now();
                    }

                    ig.fetch_sub(1, Ordering::SeqCst);
                })
            })
            .collect();

        // Wait for all threads to be ready
        while ready.load(Ordering::Acquire) < N {
            thread::yield_now();
        }
        go.store(true, Ordering::Release);

        for h in handles {
            h.join().unwrap();
        }

        // Multiple threads should have been in group lock simultaneously
        assert!(max_concurrent.load(Ordering::SeqCst) > 1, "Group locks should allow concurrency");
    }

    #[test]
    fn test_exclusive_blocks_group() {
        let mutex = Arc::new(SMutex::new());
        let exclusive_held = Arc::new(AtomicBool::new(false));
        let group_acquired = Arc::new(AtomicBool::new(false));

        let m = Arc::clone(&mutex);
        let eh = Arc::clone(&exclusive_held);
        let ga = Arc::clone(&group_acquired);

        // Thread 1: acquire exclusive lock
        let h1 = thread::spawn(move || {
            let _guard = m.lock();
            eh.store(true, Ordering::Release);

            // Hold for a while
            for _ in 0..100 {
                thread::yield_now();
            }

            // Group should not have acquired yet
            assert!(!ga.load(Ordering::Acquire), "Group should be blocked by exclusive");
        });

        // Wait for exclusive lock
        while !exclusive_held.load(Ordering::Acquire) {
            thread::yield_now();
        }

        let m = Arc::clone(&mutex);
        let ga2 = Arc::clone(&group_acquired);

        // Thread 2: try group lock (should block)
        let h2 = thread::spawn(move || {
            let _guard = m.lock_group();
            ga2.store(true, Ordering::Release);
        });

        h1.join().unwrap();
        h2.join().unwrap();

        assert!(group_acquired.load(Ordering::Acquire));
    }

    #[test]
    fn test_group_blocks_exclusive() {
        let mutex = Arc::new(SMutex::new());
        let group_held = Arc::new(AtomicBool::new(false));
        let exclusive_acquired = Arc::new(AtomicBool::new(false));

        let m = Arc::clone(&mutex);
        let gh = Arc::clone(&group_held);
        let ea = Arc::clone(&exclusive_acquired);

        // Thread 1: acquire group lock
        let h1 = thread::spawn(move || {
            let _guard = m.lock_group();
            gh.store(true, Ordering::Release);

            for _ in 0..100 {
                thread::yield_now();
            }

            // Exclusive should not have acquired yet
            assert!(!ea.load(Ordering::Acquire), "Exclusive should be blocked by group");
        });

        while !group_held.load(Ordering::Acquire) {
            thread::yield_now();
        }

        let m = Arc::clone(&mutex);
        let ea2 = Arc::clone(&exclusive_acquired);

        // Thread 2: try exclusive lock (should block)
        let h2 = thread::spawn(move || {
            let _guard = m.lock();
            ea2.store(true, Ordering::Release);
        });

        h1.join().unwrap();
        h2.join().unwrap();

        assert!(exclusive_acquired.load(Ordering::Acquire));
    }

    // ==================== Recursive Group Lock ====================

    #[test]
    fn test_exclusive_owner_can_take_group() {
        let mutex = SMutex::new();

        let guard1 = mutex.lock(); // Exclusive
        assert!(mutex.is_locked());

        let guard2 = mutex.lock_group(); // Group on top of exclusive (recursion)
        assert!(mutex.is_locked());

        drop(guard2);
        assert!(mutex.is_locked());

        drop(guard1);
        assert!(!mutex.is_locked());
    }

    // ==================== SGuard Methods ====================

    // NOTE: SGuard::unlock/lock are internal methods used by SCondVar
    // They are NOT meant for direct use as they don't track state.
    // The guard's Drop will still run, causing issues if used incorrectly.
    // These methods exist only for SCondVar's wait() implementation.

    #[test]
    fn test_guard_is_group_flag() {
        let mutex = SMutex::new();

        let exclusive = mutex.lock();
        assert!(!exclusive.is_group);
        drop(exclusive);

        let group = mutex.lock_group();
        assert!(group.is_group);
        drop(group);
    }

    #[test]
    fn test_guard_provides_mutex_reference() {
        let mutex = SMutex::new();
        let guard = mutex.lock();

        // guard.m should reference the same mutex
        assert!(std::ptr::eq(guard.m, &mutex));

        drop(guard);
    }

    // ==================== Stress Tests ====================

    #[test]
    fn test_stress_contention() {
        let mutex = Arc::new(SMutex::new());
        let counter = Arc::new(AtomicUsize::new(0));
        const N: usize = 8;
        const ITERS: usize = 50;

        let handles: Vec<_> = (0..N)
            .map(|_| {
                let m = Arc::clone(&mutex);
                let cnt = Arc::clone(&counter);
                thread::spawn(move || {
                    for _ in 0..ITERS {
                        let _guard = m.lock();
                        cnt.fetch_add(1, Ordering::Relaxed);
                        thread::yield_now();
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(counter.load(Ordering::Relaxed), N * ITERS);
    }

    #[test]
    fn test_stress_mixed_locks() {
        let mutex = Arc::new(SMutex::new());
        let exclusive_count = Arc::new(AtomicUsize::new(0));
        let group_count = Arc::new(AtomicUsize::new(0));
        const N: usize = 6;
        const ITERS: usize = 20;

        let handles: Vec<_> = (0..N)
            .map(|i| {
                let m = Arc::clone(&mutex);
                let ec = Arc::clone(&exclusive_count);
                let gc = Arc::clone(&group_count);
                thread::spawn(move || {
                    for _ in 0..ITERS {
                        if i % 2 == 0 {
                            let _guard = m.lock();
                            ec.fetch_add(1, Ordering::Relaxed);
                        } else {
                            let _guard = m.lock_group();
                            gc.fetch_add(1, Ordering::Relaxed);
                        }
                        thread::yield_now();
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        let ec = exclusive_count.load(Ordering::Relaxed);
        let gc = group_count.load(Ordering::Relaxed);
        assert_eq!(ec + gc, N * ITERS);
    }

    // ==================== Edge Cases ====================

    #[test]
    fn test_lock_after_drop() {
        let mutex = SMutex::new();

        {
            let _guard = mutex.lock();
        }

        // Should be able to lock again
        let guard = mutex.lock();
        assert!(mutex.is_locked());
        drop(guard);
    }

    #[test]
    fn test_new_mutex_state() {
        let mutex = SMutex::new();

        assert!(!mutex.is_locked());
        assert_eq!(mutex.owner.load(Ordering::Relaxed), 0);
        assert_eq!(mutex.recursion.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_unlock_wrong_thread_panics() {
        use std::panic;

        let mutex = Arc::new(SMutex::new());
        let ready = Arc::new(AtomicBool::new(false));
        let can_exit = Arc::new(AtomicBool::new(false));

        let m = Arc::clone(&mutex);
        let r = Arc::clone(&ready);
        let ce = Arc::clone(&can_exit);

        // Thread 1: lock and hold
        let h = thread::spawn(move || {
            let _guard = m.lock();
            r.store(true, Ordering::Release);

            // Wait until main thread tried to unlock
            while !ce.load(Ordering::Acquire) {
                thread::yield_now();
            }
            // Guard drops here, properly unlocking
        });

        // Wait for thread to acquire lock
        while !ready.load(Ordering::Acquire) {
            thread::yield_now();
        }

        // Try to unlock from wrong thread - should panic
        let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
            mutex.raw_unlock();
        }));

        // Signal thread to exit
        can_exit.store(true, Ordering::Release);
        h.join().unwrap();

        assert!(result.is_err(), "raw_unlock from wrong thread should panic");

        if let Err(e) = result {
            if let Some(msg) = e.downcast_ref::<&str>() {
                assert!(msg.contains("non-owner"), "Panic message should mention non-owner");
            } else if let Some(msg) = e.downcast_ref::<String>() {
                assert!(msg.contains("non-owner"), "Panic message should mention non-owner");
            }
        }
    }

    // ==================== Fairness (Basic) ====================

    #[test]
    fn test_all_threads_eventually_acquire() {
        let mutex = Arc::new(SMutex::new());
        let acquired = Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));
        const N: usize = 5;

        let handles: Vec<_> = (0..N)
            .map(|id| {
                let m = Arc::clone(&mutex);
                let acq = Arc::clone(&acquired);
                thread::spawn(move || {
                    let _guard = m.lock();
                    acq.lock().unwrap().insert(id);
                    thread::yield_now();
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        let set = acquired.lock().unwrap();
        assert_eq!(set.len(), N, "All threads should have acquired the lock");
    }
}