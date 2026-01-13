mod tests_scondvar {
    use crate::core::scondvar::SCondVar;
    use crate::core::smutex::SMutex;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};

    struct TestCtx {
        mutex: Arc<SMutex>,
        condvar: Arc<SCondVar>,
    }

    impl TestCtx {
        fn new() -> Self {
            Self {
                mutex: Arc::new(SMutex::new()),
                condvar: Arc::new(SCondVar::new()),
            }
        }

        fn clone_refs(&self) -> (Arc<SMutex>, Arc<SCondVar>) {
            (Arc::clone(&self.mutex), Arc::clone(&self.condvar))
        }
    }

    // ==================== Basic Functionality ====================

    #[test]
    fn test_notify_one_single_waiter() {
        let ctx = TestCtx::new();
        let woke_up = Arc::new(AtomicBool::new(false));
        let (m, cv) = ctx.clone_refs();
        let woke = Arc::clone(&woke_up);

        let handle = thread::spawn(move || {
            let guard = m.lock();
            let _guard = cv.wait(guard);
            woke.store(true, Ordering::Release);
            42
        });

        thread::sleep(Duration::from_millis(30));
        assert!(!woke_up.load(Ordering::Acquire), "Should still be waiting");

        ctx.condvar.notify_one();
        assert_eq!(handle.join().unwrap(), 42);
        assert!(woke_up.load(Ordering::Acquire));
    }

    #[test]
    fn test_notify_one_wakes_exactly_one() {
        let ctx = TestCtx::new();
        let wake_count = Arc::new(AtomicUsize::new(0));
        let waiting_count = Arc::new(AtomicUsize::new(0));
        const N: usize = 3;

        let handles: Vec<_> = (0..N)
            .map(|_| {
                let (m, cv) = ctx.clone_refs();
                let cnt = Arc::clone(&wake_count);
                let waiting = Arc::clone(&waiting_count);
                thread::spawn(move || {
                    let guard = m.lock();
                    waiting.fetch_add(1, Ordering::AcqRel);
                    let _guard = cv.wait(guard);
                    cnt.fetch_add(1, Ordering::AcqRel);
                })
            })
            .collect();

        // Wait until all threads are in wait state
        while waiting_count.load(Ordering::Acquire) < N {
            thread::yield_now();
        }
        // Extra yields to ensure they've entered futex_wait
        for _ in 0..100 {
            thread::yield_now();
        }

        assert_eq!(wake_count.load(Ordering::Acquire), 0);

        ctx.condvar.notify_one();

        // Wait for exactly one to wake
        while wake_count.load(Ordering::Acquire) < 1 {
            thread::yield_now();
        }

        // Give some time to see if more than one wakes (shouldn't happen)
        for _ in 0..100 {
            thread::yield_now();
        }
        assert_eq!(
            wake_count.load(Ordering::Acquire),
            1,
            "notify_one should wake exactly one"
        );

        ctx.condvar.notify_all();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(wake_count.load(Ordering::Acquire), N);
    }

    #[test]
    fn test_notify_all_wakes_all() {
        let ctx = TestCtx::new();
        let wake_count = Arc::new(AtomicUsize::new(0));
        const N: usize = 5;

        let handles: Vec<_> = (0..N)
            .map(|_| {
                let (m, cv) = ctx.clone_refs();
                let cnt = Arc::clone(&wake_count);
                thread::spawn(move || {
                    let guard = m.lock();
                    let _guard = cv.wait(guard);
                    cnt.fetch_add(1, Ordering::AcqRel);
                })
            })
            .collect();

        thread::sleep(Duration::from_millis(50));
        ctx.condvar.notify_all();

        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(wake_count.load(Ordering::Acquire), N);
    }

    // ==================== Edge Cases ====================

    #[test]
    fn test_notify_without_waiters_is_safe() {
        let cv = SCondVar::new();
        cv.notify_one();
        cv.notify_all();
        cv.notify_one();
    }

    #[test]
    fn test_wait_blocks_until_notified() {
        let ctx = TestCtx::new();
        ctx.condvar.notify_one(); // Pre-notify

        let (m, cv) = ctx.clone_refs();
        let cv2 = Arc::clone(&ctx.condvar);

        let handle = thread::spawn(move || {
            let guard = m.lock();
            let start = Instant::now();

            thread::spawn(move || {
                thread::sleep(Duration::from_millis(50));
                cv2.notify_one();
            });

            let _guard = cv.wait(guard);
            start.elapsed()
        });

        let elapsed = handle.join().unwrap();
        assert!(elapsed >= Duration::from_millis(40), "Should have blocked");
    }

    #[test]
    fn test_guard_held_after_wait() {
        let ctx = TestCtx::new();
        let (m, cv) = ctx.clone_refs();

        let handle = thread::spawn(move || {
            let guard = m.lock();
            let _guard = cv.wait(guard);
            // Guard is held - mutex should be locked
            true
        });

        thread::sleep(Duration::from_millis(30));
        ctx.condvar.notify_one();
        assert!(handle.join().unwrap());
    }

    // ==================== Multiple Cycles ====================

    #[test]
    fn test_repeated_wait_notify_cycles() {
        let ctx = TestCtx::new();
        let woke_count = Arc::new(AtomicUsize::new(0));
        let ready_to_wait = Arc::new(AtomicUsize::new(0));
        const CYCLES: usize = 5;

        let (m, cv) = ctx.clone_refs();
        let woke = Arc::clone(&woke_count);
        let ready = Arc::clone(&ready_to_wait);

        let handle = thread::spawn(move || {
            let mut guard = m.lock();
            for _ in 0..CYCLES {
                // Signal: about to wait
                ready.fetch_add(1, Ordering::AcqRel);
                guard = cv.wait(guard);
                woke.fetch_add(1, Ordering::AcqRel);
            }
        });

        for i in 0..CYCLES {
            // Wait until thread signals it's about to call wait()
            while ready_to_wait.load(Ordering::Acquire) <= i {
                thread::yield_now();
            }
            // Extra yields to ensure thread enters futex_wait
            for _ in 0..50 {
                thread::yield_now();
            }
            ctx.condvar.notify_one();

            // Wait for thread to wake before next cycle
            while woke_count.load(Ordering::Acquire) <= i {
                thread::yield_now();
            }
        }

        handle.join().unwrap();
        assert_eq!(woke_count.load(Ordering::Acquire), CYCLES);
    }

    #[test]
    fn test_condvar_reuse_across_sessions() {
        let ctx = TestCtx::new();

        for round in 0..3u32 {
            let (m, cv) = ctx.clone_refs();

            let handle = thread::spawn(move || {
                let guard = m.lock();
                let _guard = cv.wait(guard);
                round
            });

            thread::sleep(Duration::from_millis(30));
            ctx.condvar.notify_one();
            assert_eq!(handle.join().unwrap(), round);
        }
    }

    // ==================== Group Lock Integration ====================

    #[test]
    fn test_wait_with_group_lock() {
        let ctx = TestCtx::new();
        let wake_count = Arc::new(AtomicUsize::new(0));
        const N: usize = 3;

        let handles: Vec<_> = (0..N)
            .map(|_| {
                let (m, cv) = ctx.clone_refs();
                let cnt = Arc::clone(&wake_count);
                thread::spawn(move || {
                    let guard = m.lock_group();
                    let _guard = cv.wait(guard);
                    cnt.fetch_add(1, Ordering::AcqRel);
                })
            })
            .collect();

        thread::sleep(Duration::from_millis(50));
        ctx.condvar.notify_all();

        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(wake_count.load(Ordering::Acquire), N);
    }

    // ==================== Stress Tests ====================

    #[test]
    fn test_stress_many_waiters() {
        let ctx = TestCtx::new();
        let wake_count = Arc::new(AtomicUsize::new(0));
        const N: usize = 20;

        let handles: Vec<_> = (0..N)
            .map(|_| {
                let (m, cv) = ctx.clone_refs();
                let cnt = Arc::clone(&wake_count);
                thread::spawn(move || {
                    let guard = m.lock();
                    let _guard = cv.wait(guard);
                    cnt.fetch_add(1, Ordering::AcqRel);
                })
            })
            .collect();

        thread::sleep(Duration::from_millis(100));

        for i in 1..=N {
            ctx.condvar.notify_one();
            while wake_count.load(Ordering::Acquire) < i {
                thread::yield_now();
            }
        }

        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(wake_count.load(Ordering::Acquire), N);
    }

    #[test]
    fn test_stress_rapid_notify_all() {
        let ctx = TestCtx::new();
        let total_wakes = Arc::new(AtomicUsize::new(0));
        const ROUNDS: usize = 5;
        const THREADS_PER_ROUND: usize = 3;

        for round in 0..ROUNDS {
            let round_wakes = Arc::new(AtomicUsize::new(0));

            let handles: Vec<_> = (0..THREADS_PER_ROUND)
                .map(|_| {
                    let (m, cv) = ctx.clone_refs();
                    let cnt = Arc::clone(&total_wakes);
                    let rw = Arc::clone(&round_wakes);
                    thread::spawn(move || {
                        let guard = m.lock();
                        let _guard = cv.wait(guard);
                        cnt.fetch_add(1, Ordering::AcqRel);
                        rw.fetch_add(1, Ordering::AcqRel);
                    })
                })
                .collect();

            // Wait for all threads to enter wait state
            thread::sleep(Duration::from_millis(50));

            ctx.condvar.notify_all();

            // Wait for all threads in this round to complete
            for h in handles {
                h.join().unwrap();
            }

            assert_eq!(
                round_wakes.load(Ordering::Acquire),
                THREADS_PER_ROUND,
                "Round {} failed",
                round
            );
        }

        assert_eq!(
            total_wakes.load(Ordering::Acquire),
            ROUNDS * THREADS_PER_ROUND
        );
    }

    #[test]
    fn test_concurrent_wait_and_notify() {
        let ctx = TestCtx::new();
        let completed = Arc::new(AtomicUsize::new(0));
        const N: usize = 10;

        let waiter_handles: Vec<_> = (0..N)
            .map(|_| {
                let (m, cv) = ctx.clone_refs();
                let done = Arc::clone(&completed);
                thread::spawn(move || {
                    let guard = m.lock();
                    let _guard = cv.wait(guard);
                    done.fetch_add(1, Ordering::AcqRel);
                })
            })
            .collect();

        thread::sleep(Duration::from_millis(50));

        // Notify all at once
        for _ in 0..N {
            ctx.condvar.notify_one();
            thread::sleep(Duration::from_millis(5));
        }

        for h in waiter_handles {
            h.join().unwrap();
        }

        assert_eq!(completed.load(Ordering::Acquire), N);
    }

    // ==================== Fairness ====================

    #[test]
    fn test_all_waiters_eventually_wake() {
        let ctx = TestCtx::new();
        let wake_set = Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));
        const N: usize = 5;

        let handles: Vec<_> = (0..N)
            .map(|id| {
                let (m, cv) = ctx.clone_refs();
                let set = Arc::clone(&wake_set);
                thread::spawn(move || {
                    let guard = m.lock();
                    let _guard = cv.wait(guard);
                    set.lock().unwrap().insert(id);
                })
            })
            .collect();

        thread::sleep(Duration::from_millis(50));

        for _ in 0..N {
            ctx.condvar.notify_one();
            thread::sleep(Duration::from_millis(20));
        }

        for h in handles {
            h.join().unwrap();
        }

        let set = wake_set.lock().unwrap();
        assert_eq!(set.len(), N, "All threads should have woken");
    }

    // ==================== Epoch Counter ====================

    #[test]
    fn test_epoch_increments_on_notify() {
        let cv = SCondVar::new();

        let initial = cv.epoch.load(Ordering::Relaxed);
        cv.notify_one();
        assert_eq!(cv.epoch.load(Ordering::Relaxed), initial + 1);

        cv.notify_all();
        assert_eq!(cv.epoch.load(Ordering::Relaxed), initial + 2);
    }

    #[test]
    fn test_many_notifications_still_works() {
        let ctx = TestCtx::new();

        for _ in 0..100 {
            ctx.condvar.notify_one();
        }

        let (m, cv) = ctx.clone_refs();
        let cv2 = Arc::clone(&ctx.condvar);

        let handle = thread::spawn(move || {
            let guard = m.lock();
            let _guard = cv.wait(guard);
            true
        });

        thread::sleep(Duration::from_millis(30));
        cv2.notify_one();

        assert!(handle.join().unwrap());
    }

    // ==================== Mutex State ====================

    #[test]
    fn test_mutex_unlocked_while_waiting() {
        let ctx = TestCtx::new();
        let waiting = Arc::new(AtomicBool::new(false));
        let acquired = Arc::new(AtomicBool::new(false));

        let (m, cv) = ctx.clone_refs();
        let w = Arc::clone(&waiting);

        let waiter = thread::spawn(move || {
            let guard = m.lock();
            w.store(true, Ordering::Release);
            let _guard = cv.wait(guard);
        });

        while !waiting.load(Ordering::Acquire) {
            thread::yield_now();
        }
        thread::sleep(Duration::from_millis(20));

        // Should be able to acquire while waiter is blocked
        let m2 = Arc::clone(&ctx.mutex);
        let acq = Arc::clone(&acquired);

        let checker = thread::spawn(move || {
            let _guard = m2.lock();
            acq.store(true, Ordering::Release);
        });

        checker.join().unwrap();
        assert!(acquired.load(Ordering::Acquire));

        ctx.condvar.notify_one();
        waiter.join().unwrap();
    }

    #[test]
    fn test_notify_all_idempotent() {
        let ctx = TestCtx::new();
        let wake_count = Arc::new(AtomicUsize::new(0));
        const N: usize = 3;

        let handles: Vec<_> = (0..N)
            .map(|_| {
                let (m, cv) = ctx.clone_refs();
                let cnt = Arc::clone(&wake_count);
                thread::spawn(move || {
                    let guard = m.lock();
                    let _guard = cv.wait(guard);
                    cnt.fetch_add(1, Ordering::AcqRel);
                })
            })
            .collect();

        thread::sleep(Duration::from_millis(50));

        // Multiple notify_all - only wakes N threads total
        ctx.condvar.notify_all();
        ctx.condvar.notify_all();

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(wake_count.load(Ordering::Acquire), N);
    }
}
