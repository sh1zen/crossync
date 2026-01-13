mod tests_spincell {
    use crate::lock_free::SpinCell;
    use std::sync::{Arc, Barrier};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    // ==================== CONSTRUCTION & STATE ====================

    #[test]
    fn test_new() {
        let cell = SpinCell::new(42);
        assert_eq!(*cell.lock_shared(), 42);
        assert_eq!(cell.get_ref_count(), 1);
        assert!(!cell.is_locked());
        assert!(!cell.is_locked_exclusive());
        assert!(!cell.is_locked_shared());
        assert_eq!(cell.get_shared_locked(), 0);
    }

    #[test]
    fn test_state_tracking() {
        let cell = SpinCell::new(0);

        // Exclusive lock
        {
            let _guard = cell.lock_exclusive();
            assert!(cell.is_locked());
            assert!(cell.is_locked_exclusive());
            assert!(!cell.is_locked_shared());
            assert_eq!(cell.get_shared_locked(), 0);
        }
        assert!(!cell.is_locked());

        // Shared lock
        {
            let _g1 = cell.lock_shared();
            assert!(cell.is_locked());
            assert!(!cell.is_locked_exclusive());
            assert!(cell.is_locked_shared());
            assert_eq!(cell.get_shared_locked(), 1);

            let _g2 = cell.lock_shared();
            assert_eq!(cell.get_shared_locked(), 2);
        }
        assert!(!cell.is_locked());
        assert_eq!(cell.get_shared_locked(), 0);
    }

    // ==================== EXCLUSIVE LOCKING ====================

    #[test]
    fn test_exclusive_lock_unlock() {
        let cell = SpinCell::new(10);

        {
            let mut guard = cell.lock_exclusive();
            assert_eq!(*guard, 10);
            *guard = 20;
        }

        assert_eq!(*cell.lock_shared(), 20);
    }

    #[test]
    fn test_exclusive_sequential() {
        let cell = SpinCell::new(0);

        for i in 1..=5 {
            let mut g = cell.lock_exclusive();
            *g += i;
        }

        assert_eq!(*cell.lock_shared(), 15); // 1+2+3+4+5
    }

    #[test]
    fn test_exclusive_guard_deref() {
        let cell = SpinCell::new(vec![1, 2, 3]);

        {
            let mut guard = cell.lock_exclusive();
            guard.push(4);
            assert_eq!(guard.len(), 4);
        }

        assert_eq!(*cell.lock_shared(), vec![1, 2, 3, 4]);
    }

    // ==================== SHARED LOCKING ====================

    #[test]
    fn test_shared_multiple_readers() {
        let cell = SpinCell::new(100);

        let g1 = cell.lock_shared();
        let g2 = cell.lock_shared();
        let g3 = cell.lock_shared();

        assert_eq!(*g1, 100);
        assert_eq!(*g2, 100);
        assert_eq!(*g3, 100);
        assert_eq!(cell.get_shared_locked(), 3);

        drop(g1);
        assert_eq!(cell.get_shared_locked(), 2);

        drop(g2);
        drop(g3);
        assert_eq!(cell.get_shared_locked(), 0);
    }

    #[test]
    fn test_unlock_all_shared() {
        let cell = SpinCell::new(42);

        // Acquisisce lock shared e dimentica i guard per evitare unlock automatico
        let g1 = cell.lock_shared();
        let g2 = cell.lock_shared();
        let g3 = cell.lock_shared();
        assert_eq!(cell.get_shared_locked(), 3);

        // Dimentica i guard per evitare double-unlock
        std::mem::forget(g1);
        std::mem::forget(g2);
        std::mem::forget(g3);

        // unlock_all_shared deve rilasciare tutti i lock
        cell.unlock_all_shared();
        assert_eq!(cell.get_shared_locked(), 0);
        assert!(!cell.is_locked());
    }

    // ==================== DEBUG ====================

    #[test]
    fn test_debug_format() {
        let cell = SpinCell::new(42);
        let debug = format!("{:?}", cell);

        assert!(debug.contains("SpinLockCell"));
        assert!(debug.contains("state"));
        assert!(debug.contains("exclusive_locked"));
        assert!(debug.contains("readers_count"));
    }

    // ==================== CONCURRENT - EXCLUSIVE ====================

    #[test]
    fn test_concurrent_exclusive_increment() {
        let cell = Arc::new(SpinCell::new(0usize));
        let barrier = Arc::new(Barrier::new(4));

        let handles: Vec<_> = (0..4)
            .map(|_| {
                let c = cell.clone();
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                    for _ in 0..500 {
                        let mut g = c.lock_exclusive();
                        *g += 1;
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(*cell.lock_shared(), 2000);
    }

    #[test]
    fn test_concurrent_exclusive_complex_type() {
        let cell = Arc::new(SpinCell::new(Vec::<i32>::new()));
        let barrier = Arc::new(Barrier::new(4));

        let handles: Vec<_> = (0..4)
            .map(|t| {
                let c = cell.clone();
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                    for i in 0..100 {
                        let mut g = c.lock_exclusive();
                        g.push(t * 100 + i);
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(cell.lock_shared().len(), 400);
    }

    // ==================== CONCURRENT - SHARED ====================

    #[test]
    fn test_concurrent_shared_readers() {
        let cell = Arc::new(SpinCell::new(42));
        let barrier = Arc::new(Barrier::new(8));
        let sum = Arc::new(AtomicUsize::new(0));

        let handles: Vec<_> = (0..8)
            .map(|_| {
                let c = cell.clone();
                let b = barrier.clone();
                let s = sum.clone();
                thread::spawn(move || {
                    b.wait();
                    for _ in 0..1000 {
                        let g = c.lock_shared();
                        s.fetch_add(*g, Ordering::Relaxed);
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(sum.load(Ordering::SeqCst), 42 * 8 * 1000);
    }

    // ==================== CONCURRENT - MIXED ====================

    #[test]
    fn test_concurrent_readers_writers() {
        let cell = Arc::new(SpinCell::new(0i64));
        let barrier = Arc::new(Barrier::new(8));

        // 4 writers
        let writers: Vec<_> = (0..4)
            .map(|_| {
                let c = cell.clone();
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                    for _ in 0..200 {
                        let mut g = c.lock_exclusive();
                        *g += 1;
                    }
                })
            })
            .collect();

        // 4 readers
        let readers: Vec<_> = (0..4)
            .map(|_| {
                let c = cell.clone();
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                    for _ in 0..500 {
                        let g = c.lock_shared();
                        let _ = *g; // Just read
                    }
                })
            })
            .collect();

        for h in writers {
            h.join().unwrap();
        }
        for h in readers {
            h.join().unwrap();
        }

        assert_eq!(*cell.lock_shared(), 800); // 4 writers * 200
    }

    #[test]
    fn test_concurrent_heavy_contention() {
        let cell = Arc::new(SpinCell::new(0i64));
        let barrier = Arc::new(Barrier::new(10));

        let handles: Vec<_> = (0..10)
            .map(|t| {
                let c = cell.clone();
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                    for i in 0..100 {
                        if (t + i) % 3 == 0 {
                            // Write
                            let mut g = c.lock_exclusive();
                            *g += 1;
                        } else {
                            // Read
                            let _g = c.lock_shared();
                        }
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        // Valore finale dipende dal pattern, ma deve essere consistente
        let final_val = *cell.lock_shared();
        assert!(final_val > 0, "Nessuna scrittura avvenuta");
    }

    // ==================== FAIRNESS & STARVATION ====================

    #[test]
    fn test_writer_not_starved() {
        let cell = Arc::new(SpinCell::new(0));
        let writer_done = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(5));

        // 4 reader threads che acquisiscono continuamente
        let readers: Vec<_> = (0..4)
            .map(|_| {
                let c = cell.clone();
                let done = writer_done.clone();
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                    while done.load(Ordering::Relaxed) == 0 {
                        let _g = c.lock_shared();
                        thread::yield_now();
                    }
                })
            })
            .collect();

        // 1 writer thread
        let c = cell.clone();
        let done = writer_done.clone();
        let b = barrier.clone();
        let writer = thread::spawn(move || {
            b.wait();
            for _ in 0..10 {
                let mut g = c.lock_exclusive();
                *g += 1;
            }
            done.store(1, Ordering::Release);
        });

        writer.join().unwrap();
        for r in readers {
            r.join().unwrap();
        }

        assert_eq!(*cell.lock_shared(), 10);
    }

    // ==================== EDGE CASES ====================

    #[test]
    fn test_zero_sized_type() {
        let cell = SpinCell::new(());

        {
            let _g = cell.lock_exclusive();
        }

        {
            let _g1 = cell.lock_shared();
            let _g2 = cell.lock_shared();
        }
    }

    #[test]
    fn test_large_value() {
        let large_data = vec![0u8; 1024 * 1024]; // 1MB
        let cell = SpinCell::new(large_data);

        {
            let mut g = cell.lock_exclusive();
            g[0] = 42;
        }

        assert_eq!(cell.lock_shared()[0], 42);
    }

    #[test]
    fn test_nested_data() {
        let cell = SpinCell::new(Arc::new(AtomicUsize::new(0)));

        let inner = {
            let g = cell.lock_shared();
            g.clone()
        };

        inner.fetch_add(1, Ordering::SeqCst);

        {
            let g = cell.lock_shared();
            assert_eq!(g.load(Ordering::SeqCst), 1);
        }
    }

    // ==================== GUARD BEHAVIOR ====================

    #[test]
    fn test_guard_drop_releases_lock() {
        let cell = SpinCell::new(0);

        let guard = cell.lock_exclusive();
        assert!(cell.is_locked_exclusive());
        drop(guard);
        assert!(!cell.is_locked());

        let guard = cell.lock_shared();
        assert!(cell.is_locked_shared());
        drop(guard);
        assert!(!cell.is_locked());
    }

    #[test]
    fn test_shared_guard_deref() {
        let cell = SpinCell::new("hello".to_string());

        let guard = cell.lock_shared();
        assert_eq!(guard.len(), 5);
        assert!(guard.starts_with("hel"));
    }

    #[test]
    fn test_exclusive_guard_deref_mut() {
        let cell = SpinCell::new(vec![1, 2]);

        {
            let mut guard = cell.lock_exclusive();
            guard.push(3);
            guard[0] = 10;
        }

        assert_eq!(*cell.lock_shared(), vec![10, 2, 3]);
    }

    // ==================== RAPID LOCK/UNLOCK ====================

    #[test]
    fn test_rapid_lock_unlock_exclusive() {
        let cell = SpinCell::new(0);

        for _ in 0..1000 {
            let mut g = cell.lock_exclusive();
            *g += 1;
        }

        assert_eq!(*cell.lock_shared(), 1000);
    }

    #[test]
    fn test_rapid_lock_unlock_shared() {
        let cell = SpinCell::new(42);

        for _ in 0..1000 {
            let g = cell.lock_shared();
            assert_eq!(*g, 42);
        }
    }

    #[test]
    fn test_alternating_exclusive_shared() {
        let cell = SpinCell::new(0);

        for i in 0..100 {
            if i % 2 == 0 {
                let mut g = cell.lock_exclusive();
                *g += 1;
            } else {
                let g = cell.lock_shared();
                let _ = *g;
            }
        }

        assert_eq!(*cell.lock_shared(), 50);
    }
}
