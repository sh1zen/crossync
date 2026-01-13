mod tests_rawmutex {
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex as StdMutex;
    use std::thread;
    use crate::sync::RawMutex;
    // ==================== Basic State ====================

    #[test]
    fn test_new_mutex_state() {
        let m = RawMutex::new();
        assert!(!m.is_locked());
        assert!(!m.is_locked_exclusive());
        assert!(!m.is_locked_shared());
        assert_eq!(m.get_shared_locked(), 0);
        assert_eq!(m.get_ref_count(), 1);
    }

    #[test]
    fn test_default_creates_unlocked() {
        let m = RawMutex::default();
        assert!(!m.is_locked());
    }

    #[test]
    fn test_debug_format() {
        let m = RawMutex::new();
        let debug_str = format!("{:?}", m);
        assert!(debug_str.contains("Mutex"));
        assert!(debug_str.contains("exclusive_locked"));
        assert!(debug_str.contains("readers_count"));
    }

    // ==================== Exclusive Lock ====================

    #[test]
    fn test_exclusive_lock_unlock() {
        let m = RawMutex::new();

        assert!(!m.is_locked_exclusive());
        m.lock_exclusive();
        assert!(m.is_locked_exclusive());
        assert!(m.is_locked());

        m.unlock_exclusive();
        assert!(!m.is_locked_exclusive());
        assert!(!m.is_locked());
    }

    #[test]
    fn test_try_lock_exclusive_success() {
        let m = RawMutex::new();

        assert!(m.try_lock_exclusive());
        assert!(m.is_locked_exclusive());

        m.unlock_exclusive();
    }

    #[test]
    fn test_try_lock_exclusive_fails_when_locked() {
        let m = RawMutex::new();

        m.lock_exclusive();

        let m2 = m.clone();
        let result = thread::spawn(move || m2.try_lock_exclusive()).join().unwrap();

        assert!(!result, "try_lock_exclusive should fail when already locked");

        m.unlock_exclusive();
    }

    #[test]
    fn test_exclusive_mutual_exclusion() {
        let m = RawMutex::new();
        let in_critical = AtomicBool::new(false);
        let violation = AtomicBool::new(false);
        const N: usize = 4;
        const ITERS: usize = 50;

        thread::scope(|s| {
            for _ in 0..N {
                let mm = m.clone();
                let ic = &in_critical;
                let v = &violation;
                s.spawn(move || {
                    for _ in 0..ITERS {
                        mm.lock_exclusive();
                        if ic.swap(true, Ordering::AcqRel) {
                            v.store(true, Ordering::Release);
                        }
                        thread::yield_now();
                        ic.store(false, Ordering::Release);
                        mm.unlock_exclusive();
                    }
                });
            }
        });

        assert!(!violation.load(Ordering::Acquire), "Mutual exclusion violated");
    }

    // ==================== Shared Lock ====================

    #[test]
    fn test_shared_lock_unlock() {
        let m = RawMutex::new();

        assert!(!m.is_locked_shared());
        m.lock_shared();
        assert!(m.is_locked_shared());
        assert!(m.is_locked());
        assert_eq!(m.get_shared_locked(), 1);

        m.unlock_shared();
        assert!(!m.is_locked_shared());
        assert!(!m.is_locked());
        assert_eq!(m.get_shared_locked(), 0);
    }

    #[test]
    fn test_multiple_shared_locks() {
        let m = RawMutex::new();

        m.lock_shared();
        assert_eq!(m.get_shared_locked(), 1);

        m.lock_shared();
        assert_eq!(m.get_shared_locked(), 2);

        m.lock_shared();
        assert_eq!(m.get_shared_locked(), 3);

        m.unlock_shared();
        assert_eq!(m.get_shared_locked(), 2);

        m.unlock_shared();
        assert_eq!(m.get_shared_locked(), 1);

        m.unlock_shared();
        assert_eq!(m.get_shared_locked(), 0);
    }

    #[test]
    fn test_try_lock_shared_success() {
        let m = RawMutex::new();

        assert!(m.try_lock_shared());
        assert!(m.is_locked_shared());
        assert_eq!(m.get_shared_locked(), 1);

        // Can acquire more shared locks
        assert!(m.try_lock_shared());
        assert_eq!(m.get_shared_locked(), 2);

        m.unlock_shared();
        m.unlock_shared();
    }

    #[test]
    fn test_try_lock_shared_fails_when_exclusive() {
        let m = RawMutex::new();

        m.lock_exclusive();

        let m2 = m.clone();
        let result = thread::spawn(move || m2.try_lock_shared()).join().unwrap();

        assert!(!result, "try_lock_shared should fail when exclusive locked");

        m.unlock_exclusive();
    }

    #[test]
    fn test_shared_allows_concurrency() {
        let m = RawMutex::new();
        let concurrent = AtomicUsize::new(0);
        let max_concurrent = AtomicUsize::new(0);
        let ready = AtomicUsize::new(0);
        const N: usize = 5;

        thread::scope(|s| {
            for _ in 0..N {
                let mm = m.clone();
                let cur = &concurrent;
                let maxc = &max_concurrent;
                let r = &ready;
                s.spawn(move || {
                    mm.lock_shared();
                    r.fetch_add(1, Ordering::AcqRel);

                    // Wait for all to acquire
                    while r.load(Ordering::Acquire) < N {
                        thread::yield_now();
                    }

                    let now = cur.fetch_add(1, Ordering::AcqRel) + 1;
                    maxc.fetch_max(now, Ordering::AcqRel);

                    for _ in 0..20 {
                        thread::yield_now();
                    }

                    cur.fetch_sub(1, Ordering::AcqRel);
                    mm.unlock_shared();
                });
            }
        });

        assert!(
            max_concurrent.load(Ordering::Acquire) > 1,
            "Shared locks should allow concurrency"
        );
    }

    // ==================== Exclusive vs Shared Interaction ====================

    #[test]
    fn test_exclusive_blocks_shared() {
        let m = RawMutex::new();
        let shared_acquired = AtomicBool::new(false);
        let exclusive_released = AtomicBool::new(false);

        m.lock_exclusive();

        thread::scope(|s| {
            let mm = m.clone();
            let sa = &shared_acquired;
            let er = &exclusive_released;
            s.spawn(move || {
                mm.lock_shared();
                sa.store(true, Ordering::Release);
                assert!(
                    er.load(Ordering::Acquire),
                    "Shared acquired before exclusive released"
                );
                mm.unlock_shared();
            });

            // Give thread time to block
            for _ in 0..50 {
                thread::yield_now();
            }
            assert!(!shared_acquired.load(Ordering::Acquire));

            exclusive_released.store(true, Ordering::Release);
            m.unlock_exclusive();
        });

        assert!(shared_acquired.load(Ordering::Acquire));
    }

    #[test]
    fn test_shared_blocks_exclusive() {
        let m = RawMutex::new();
        let exclusive_acquired = AtomicBool::new(false);
        let shared_released = AtomicBool::new(false);

        m.lock_shared();

        thread::scope(|s| {
            let mm = m.clone();
            let ea = &exclusive_acquired;
            let sr = &shared_released;
            s.spawn(move || {
                mm.lock_exclusive();
                ea.store(true, Ordering::Release);
                assert!(
                    sr.load(Ordering::Acquire),
                    "Exclusive acquired before shared released"
                );
                mm.unlock_exclusive();
            });

            for _ in 0..50 {
                thread::yield_now();
            }
            assert!(!exclusive_acquired.load(Ordering::Acquire));

            shared_released.store(true, Ordering::Release);
            m.unlock_shared();
        });

        assert!(exclusive_acquired.load(Ordering::Acquire));
    }

    // ==================== unlock_all_shared ====================

    #[test]
    fn test_unlock_all_shared() {
        let m = RawMutex::new();

        m.lock_shared();
        m.lock_shared();
        m.lock_shared();
        assert_eq!(m.get_shared_locked(), 3);

        m.unlock_all_shared();
        assert_eq!(m.get_shared_locked(), 0);
        assert!(!m.is_locked());
    }

    #[test]
    fn test_unlock_all_shared_wakes_writer() {
        let m = RawMutex::new();
        let writer_acquired = AtomicBool::new(false);

        m.lock_shared();
        m.lock_shared();

        thread::scope(|s| {
            let mm = m.clone();
            let wa = &writer_acquired;
            s.spawn(move || {
                mm.lock_exclusive();
                wa.store(true, Ordering::Release);
                mm.unlock_exclusive();
            });

            for _ in 0..50 {
                thread::yield_now();
            }
            assert!(!writer_acquired.load(Ordering::Acquire));

            m.unlock_all_shared();
        });

        assert!(writer_acquired.load(Ordering::Acquire));
    }

    // ==================== Downgrade ====================

    #[test]
    fn test_downgrade_exclusive_to_shared() {
        let m = RawMutex::new();

        m.lock_exclusive();
        assert!(m.is_locked_exclusive());
        assert!(!m.is_locked_shared());

        m.downgrade();
        assert!(!m.is_locked_exclusive());
        assert!(m.is_locked_shared());
        assert_eq!(m.get_shared_locked(), 1);

        m.unlock_shared();
        assert!(!m.is_locked());
    }

    #[test]
    fn test_downgrade_allows_other_readers() {
        let m = RawMutex::new();
        let other_reader_acquired = AtomicBool::new(false);

        m.lock_exclusive();

        thread::scope(|s| {
            let mm = m.clone();
            let ora = &other_reader_acquired;
            s.spawn(move || {
                mm.lock_shared();
                ora.store(true, Ordering::Release);
                mm.unlock_shared();
            });

            for _ in 0..50 {
                thread::yield_now();
            }
            assert!(!other_reader_acquired.load(Ordering::Acquire));

            m.downgrade();

            // Now the other reader should be able to acquire
            while !other_reader_acquired.load(Ordering::Acquire) {
                thread::yield_now();
            }

            m.unlock_shared();
        });

        assert!(other_reader_acquired.load(Ordering::Acquire));
    }

    // ==================== Reference Counting ====================

    #[test]
    fn test_clone_increments_ref_count() {
        let m = RawMutex::new();
        assert_eq!(m.get_ref_count(), 1);

        let m2 = m.clone();
        assert_eq!(m.get_ref_count(), 2);
        assert_eq!(m2.get_ref_count(), 2);

        let m3 = m.clone();
        assert_eq!(m.get_ref_count(), 3);

        drop(m3);
        assert_eq!(m.get_ref_count(), 2);

        drop(m2);
        assert_eq!(m.get_ref_count(), 1);
    }

    #[test]
    fn test_clones_share_lock_state() {
        let m1 = RawMutex::new();
        let m2 = m1.clone();

        m1.lock_exclusive();
        assert!(m2.is_locked_exclusive());

        m2.unlock_exclusive();
        assert!(!m1.is_locked_exclusive());
    }

    // ==================== Stress Tests ====================

    #[test]
    fn test_stress_mixed_locks() {
        let m = RawMutex::new();
        let counter = AtomicUsize::new(0);
        const N: usize = 8;
        const ITERS: usize = 100;

        thread::scope(|s| {
            for id in 0..N {
                let mm = m.clone();
                let cnt = &counter;
                s.spawn(move || {
                    for i in 0..ITERS {
                        if (id + i) % 3 == 0 {
                            mm.lock_exclusive();
                            cnt.fetch_add(1, Ordering::Relaxed);
                            mm.unlock_exclusive();
                        } else {
                            mm.lock_shared();
                            cnt.fetch_add(1, Ordering::Relaxed);
                            mm.unlock_shared();
                        }
                    }
                });
            }
        });

        assert_eq!(counter.load(Ordering::Relaxed), N * ITERS);
    }

    #[test]
    fn test_stress_readers_with_occasional_writer() {
        let m = RawMutex::new();
        let read_count = AtomicUsize::new(0);
        let write_count = AtomicUsize::new(0);
        const READERS: usize = 6;
        const WRITERS: usize = 2;
        const ITERS: usize = 50;

        thread::scope(|s| {
            // Readers
            for _ in 0..READERS {
                let mm = m.clone();
                let rc = &read_count;
                s.spawn(move || {
                    for _ in 0..ITERS {
                        mm.lock_shared();
                        rc.fetch_add(1, Ordering::Relaxed);
                        thread::yield_now();
                        mm.unlock_shared();
                    }
                });
            }

            // Writers
            for _ in 0..WRITERS {
                let mm = m.clone();
                let wc = &write_count;
                s.spawn(move || {
                    for _ in 0..ITERS {
                        mm.lock_exclusive();
                        wc.fetch_add(1, Ordering::Relaxed);
                        thread::yield_now();
                        mm.unlock_exclusive();
                    }
                });
            }
        });

        assert_eq!(read_count.load(Ordering::Relaxed), READERS * ITERS);
        assert_eq!(write_count.load(Ordering::Relaxed), WRITERS * ITERS);
    }

    #[test]
    fn test_stress_contention() {
        let m = RawMutex::new();
        let data = AtomicUsize::new(0);
        const N: usize = 10;
        const ITERS: usize = 100;

        thread::scope(|s| {
            for _ in 0..N {
                let mm = m.clone();
                let d = &data;
                s.spawn(move || {
                    for _ in 0..ITERS {
                        mm.lock_exclusive();
                        let val = d.load(Ordering::Relaxed);
                        thread::yield_now();
                        d.store(val + 1, Ordering::Relaxed);
                        mm.unlock_exclusive();
                    }
                });
            }
        });

        assert_eq!(data.load(Ordering::Relaxed), N * ITERS);
    }

    // ==================== Fairness ====================

    #[test]
    fn test_all_threads_eventually_acquire_exclusive() {
        let m = RawMutex::new();
        let acquired = StdMutex::new(HashSet::new());
        const N: usize = 5;

        thread::scope(|s| {
            for id in 0..N {
                let mm = m.clone();
                let acq = &acquired;
                s.spawn(move || {
                    mm.lock_exclusive();
                    acq.lock().unwrap().insert(id);
                    thread::yield_now();
                    mm.unlock_exclusive();
                });
            }
        });

        let set = acquired.lock().unwrap();
        assert_eq!(set.len(), N, "All threads should have acquired the lock");
    }

    #[test]
    fn test_all_threads_eventually_acquire_shared() {
        let m = RawMutex::new();
        let acquired = StdMutex::new(HashSet::new());
        const N: usize = 10;

        thread::scope(|s| {
            for id in 0..N {
                let mm = m.clone();
                let acq = &acquired;
                s.spawn(move || {
                    mm.lock_shared();
                    acq.lock().unwrap().insert(id);
                    thread::yield_now();
                    mm.unlock_shared();
                });
            }
        });

        let set = acquired.lock().unwrap();
        assert_eq!(set.len(), N, "All threads should have acquired shared lock");
    }

    // ==================== Edge Cases ====================

    #[test]
    fn test_lock_after_full_release() {
        let m = RawMutex::new();

        m.lock_exclusive();
        m.unlock_exclusive();

        m.lock_shared();
        m.unlock_shared();

        m.lock_exclusive();
        m.downgrade();
        m.unlock_shared();

        // Should be able to lock again
        assert!(m.try_lock_exclusive());
        m.unlock_exclusive();
    }

    #[test]
    fn test_rapid_lock_unlock() {
        let m = RawMutex::new();

        for _ in 0..1000 {
            m.lock_exclusive();
            m.unlock_exclusive();
        }

        assert!(!m.is_locked());
    }

    #[test]
    fn test_rapid_shared_lock_unlock() {
        let m = RawMutex::new();

        for _ in 0..1000 {
            m.lock_shared();
            m.unlock_shared();
        }

        assert!(!m.is_locked());
    }

    // ==================== Cross-thread Operations ====================

    #[test]
    fn test_lock_on_one_thread_unlock_on_another() {
        let m = RawMutex::new();

        m.lock_exclusive();

        let m2 = m.clone();
        thread::spawn(move || {
            m2.unlock_exclusive();
        })
            .join()
            .unwrap();

        assert!(!m.is_locked());
    }

    #[test]
    fn test_shared_lock_unlock_different_threads() {
        let m = RawMutex::new();

        m.lock_shared();
        m.lock_shared();

        let m2 = m.clone();
        thread::spawn(move || {
            m2.unlock_shared();
        })
            .join()
            .unwrap();

        assert_eq!(m.get_shared_locked(), 1);

        m.unlock_shared();
        assert!(!m.is_locked());
    }

    // ==================== Data Protection ====================

    #[test]
    fn test_exclusive_protects_data_integrity() {
        let m = RawMutex::new();
        let data = AtomicUsize::new(0);
        const N: usize = 5;
        const ITERS: usize = 100;

        thread::scope(|s| {
            for _ in 0..N {
                let mm = m.clone();
                let d = &data;
                s.spawn(move || {
                    for _ in 0..ITERS {
                        mm.lock_exclusive();
                        // Non-atomic read-modify-write should be safe under exclusive lock
                        let val = d.load(Ordering::Relaxed);
                        thread::yield_now(); // Context switch opportunity
                        d.store(val + 1, Ordering::Relaxed);
                        mm.unlock_exclusive();
                    }
                });
            }
        });

        assert_eq!(
            data.load(Ordering::Relaxed),
            N * ITERS,
            "Data corrupted - exclusive lock didn't provide mutual exclusion"
        );
    }
}