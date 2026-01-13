
#[cfg(test)]
mod tests_barrier {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};
    use crate::sync::Barrier;
    // ==================== CONSTRUCTION ====================

    #[test]
    fn test_new() {
        let barrier = Barrier::new();
        assert_eq!(barrier.count(), 1);
    }

    #[test]
    fn test_default() {
        let barrier = Barrier::default();
        assert_eq!(barrier.count(), 1);
    }

    #[test]
    fn test_with_capacity() {
        let barrier = Barrier::with_capacity(5, 0);
        assert_eq!(barrier.count(), 7); // n + 2

        let barrier_reusable = Barrier::with_capacity(3, 3);
        assert_eq!(barrier_reusable.count(), 5); // n + 2
    }

    // ==================== SINGLE THREAD ====================

    #[test]
    fn test_single_thread_no_block() {
        let barrier = Barrier::new();

        let start = Instant::now();
        barrier.wait();
        let elapsed = start.elapsed();

        // Non deve bloccare
        assert!(elapsed < Duration::from_millis(100));
        assert_eq!(barrier.count(), 1);
    }

    #[test]
    fn test_single_clone_no_block() {
        let barrier = Barrier::with_capacity(5, 0);
        // Solo un clone, ref_count == 1 dopo drop dell'originale

        // wait() ritorna subito se ref_count == 1
        barrier.wait();
    }

    // ==================== CLONE & REFCOUNT ====================

    #[test]
    fn test_clone() {
        let barrier = Barrier::with_capacity(2, 0);
        let clone = barrier.clone();

        // Entrambi puntano allo stesso inner
        assert_eq!(barrier.count(), clone.count());
    }

    #[test]
    fn test_clone_multiple() {
        let barrier = Barrier::with_capacity(3, 0);
        let c1 = barrier.clone();
        let c2 = barrier.clone();
        let c3 = c1.clone();

        drop(c1);
        drop(c2);

        // c3 e barrier ancora validi
        assert_eq!(c3.count(), barrier.count());
    }

    #[test]
    fn test_drop_releases_memory() {
        let barrier = Barrier::with_capacity(2, 0);
        let c1 = barrier.clone();
        let c2 = barrier.clone();

        drop(barrier);
        drop(c1);
        drop(c2);
        // Se arriviamo qui senza crash, la memoria è stata gestita correttamente
    }

    // ==================== BASIC SYNCHRONIZATION ====================

    #[test]
    fn test_two_threads_sync() {
        let barrier = Barrier::with_capacity(2, 0);
        let counter = Arc::new(AtomicUsize::new(0));

        let b = barrier.clone();
        let c = counter.clone();
        let t1 = thread::spawn(move || {
            c.fetch_add(1, Ordering::SeqCst);
            b.wait();
            c.fetch_add(10, Ordering::SeqCst);
        });

        let b = barrier.clone();
        let c = counter.clone();
        let t2 = thread::spawn(move || {
            c.fetch_add(1, Ordering::SeqCst);
            b.wait();
            c.fetch_add(10, Ordering::SeqCst);
        });

        t1.join().unwrap();
        t2.join().unwrap();

        assert_eq!(counter.load(Ordering::SeqCst), 22);
    }

    #[test]
    fn test_three_threads_sync() {
        let barrier = Barrier::with_capacity(3, 0);
        let counter = Arc::new(AtomicUsize::new(0));

        let handles: Vec<_> = (0..3)
            .map(|_| {
                let b = barrier.clone();
                let c = counter.clone();
                thread::spawn(move || {
                    c.fetch_add(1, Ordering::SeqCst);
                    b.wait();
                    c.fetch_add(1, Ordering::SeqCst);
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(counter.load(Ordering::SeqCst), 6);
    }

    // ==================== REUSABLE BARRIER ====================

    #[test]
    fn test_barrier_reuse() {
        let barrier = Barrier::with_capacity(2, 2);
        let counter = Arc::new(AtomicUsize::new(0));

        // Prima ondata
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let b = barrier.clone();
                let c = counter.clone();
                thread::spawn(move || {
                    c.fetch_add(1, Ordering::SeqCst);
                    b.wait();
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(counter.load(Ordering::SeqCst), 2);

        // Seconda ondata (barrier riutilizzabile)
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let b = barrier.clone();
                let c = counter.clone();
                thread::spawn(move || {
                    c.fetch_add(1, Ordering::SeqCst);
                    b.wait();
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(counter.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn test_barrier_disabled_after_use() {
        let barrier = Barrier::with_capacity(2, 0); // bucket = 0 -> disabilitato dopo uso

        let handles: Vec<_> = (0..2)
            .map(|_| {
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        // Dopo l'uso, count dovrebbe essere 0 (disabilitato)
        assert_eq!(barrier.count(), 0);
    }

    // ==================== RELEASE ====================

    #[test]
    fn test_manual_release() {
        let barrier = Barrier::with_capacity(10, 0);
        let released = Arc::new(AtomicUsize::new(0));

        // Avvia thread che aspettano
        let handles: Vec<_> = (0..3)
            .map(|_| {
                let b = barrier.clone();
                let r = released.clone();
                thread::spawn(move || {
                    b.wait();
                    r.fetch_add(1, Ordering::SeqCst);
                })
            })
            .collect();

        // Dai tempo ai thread di arrivare al wait
        thread::sleep(Duration::from_millis(50));

        // Release manuale
        barrier.release();

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(released.load(Ordering::SeqCst), 3);
    }

    // ==================== CONCURRENT STRESS ====================

    #[test]
    fn test_many_threads() {
        let barrier = Barrier::with_capacity(10, 0);
        let counter = Arc::new(AtomicUsize::new(0));

        let handles: Vec<_> = (0..10)
            .map(|_| {
                let b = barrier.clone();
                let c = counter.clone();
                thread::spawn(move || {
                    c.fetch_add(1, Ordering::SeqCst);
                    b.wait();
                    c.fetch_add(1, Ordering::SeqCst);
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(counter.load(Ordering::SeqCst), 20);
    }

    #[test]
    fn test_threads_with_delays() {
        let barrier = Barrier::with_capacity(4, 0);
        let order = Arc::new(AtomicUsize::new(0));

        let handles: Vec<_> = (0..4)
            .map(|i| {
                let b = barrier.clone();
                let o = order.clone();
                thread::spawn(move || {
                    // Thread diversi arrivano in momenti diversi
                    thread::sleep(Duration::from_millis(i as u64 * 10));
                    o.fetch_add(1, Ordering::SeqCst);
                    b.wait();
                    // Dopo la barriera, tutti proseguono
                    o.fetch_add(10, Ordering::SeqCst);
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(order.load(Ordering::SeqCst), 44); // 4 + 40
    }

    #[test]
    fn test_concurrent_clone_and_wait() {
        let barrier = Arc::new(Barrier::with_capacity(5, 0));
        let counter = Arc::new(AtomicUsize::new(0));

        let handles: Vec<_> = (0..5)
            .map(|_| {
                let b = (*barrier).clone();
                let c = counter.clone();
                thread::spawn(move || {
                    c.fetch_add(1, Ordering::SeqCst);
                    b.wait();
                    c.fetch_add(1, Ordering::SeqCst);
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(counter.load(Ordering::SeqCst), 10);
    }

    // ==================== EDGE CASES ====================

    #[test]
    fn test_zero_capacity() {
        // with_capacity(0, 0) crea barrier con n=2
        let barrier = Barrier::with_capacity(0, 0);
        assert_eq!(barrier.count(), 2);
    }

    #[test]
    fn test_large_capacity() {
        let barrier = Barrier::with_capacity(100, 0);
        assert_eq!(barrier.count(), 102);
    }

    #[test]
    fn test_wait_after_disabled() {
        let barrier = Barrier::with_capacity(2, 0);

        // Prima usa la barrier normalmente
        let b1 = barrier.clone();
        let b2 = barrier.clone();

        let t1 = thread::spawn(move || b1.wait());
        let t2 = thread::spawn(move || b2.wait());

        t1.join().unwrap();
        t2.join().unwrap();

        // Ora la barrier è disabilitata (count == 0)
        assert_eq!(barrier.count(), 0);

        // wait() su barrier disabilitata non deve bloccare
        barrier.wait();
    }

    #[test]
    fn test_multiple_barriers_independent() {
        let barrier1 = Barrier::with_capacity(2, 0);
        let barrier2 = Barrier::with_capacity(2, 0);
        let counter = Arc::new(AtomicUsize::new(0));

        // Thread che usano barrier1
        let b1a = barrier1.clone();
        let b1b = barrier1.clone();
        let c1 = counter.clone();
        let c2 = counter.clone();

        let t1 = thread::spawn(move || {
            c1.fetch_add(1, Ordering::SeqCst);
            b1a.wait();
        });

        let t2 = thread::spawn(move || {
            c2.fetch_add(1, Ordering::SeqCst);
            b1b.wait();
        });

        t1.join().unwrap();
        t2.join().unwrap();

        // Thread che usano barrier2
        let b2a = barrier2.clone();
        let b2b = barrier2.clone();
        let c3 = counter.clone();
        let c4 = counter.clone();

        let t3 = thread::spawn(move || {
            c3.fetch_add(10, Ordering::SeqCst);
            b2a.wait();
        });

        let t4 = thread::spawn(move || {
            c4.fetch_add(10, Ordering::SeqCst);
            b2b.wait();
        });

        t3.join().unwrap();
        t4.join().unwrap();

        assert_eq!(counter.load(Ordering::SeqCst), 22);
    }

    // ==================== SYNCHRONIZATION GUARANTEE ====================

    #[test]
    fn test_all_threads_reach_barrier_before_continuing() {
        let barrier = Barrier::with_capacity(4, 0);
        let before_barrier = Arc::new(AtomicUsize::new(0));
        let after_barrier = Arc::new(AtomicUsize::new(0));

        let handles: Vec<_> = (0..4)
            .map(|_| {
                let b = barrier.clone();
                let before = before_barrier.clone();
                let after = after_barrier.clone();
                thread::spawn(move || {
                    before.fetch_add(1, Ordering::SeqCst);
                    b.wait();
                    // A questo punto tutti i thread devono aver incrementato before
                    let before_val = before.load(Ordering::SeqCst);
                    assert_eq!(before_val, 4, "Not all threads reached barrier");
                    after.fetch_add(1, Ordering::SeqCst);
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(before_barrier.load(Ordering::SeqCst), 4);
        assert_eq!(after_barrier.load(Ordering::SeqCst), 4);
    }
}