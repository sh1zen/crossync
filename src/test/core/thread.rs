mod tests_thread {
    use crate::core::thread::ThreadParker;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};

    // ==================== BASIC FUNCTIONALITY ====================

    #[test]
    fn test_new() {
        let (parker, unparker) = ThreadParker::new();
        // Dovrebbe avere un token iniziale (UNPARKED = 1)
        drop(parker);
        drop(unparker);
    }

    #[test]
    fn test_unpark_before_park() {
        let (parker, unparker) = ThreadParker::new();

        // Unpark prima di park -> park non blocca
        unparker.unpark();

        let start = Instant::now();
        parker.park();
        let elapsed = start.elapsed();

        assert!(elapsed < Duration::from_millis(100), "park() blocked unexpectedly");
    }

    #[test]
    fn test_multiple_tokens() {
        let (parker, unparker) = ThreadParker::new();

        // Accumula token
        unparker.unpark();
        unparker.unpark();
        unparker.unpark();

        // Consuma token senza bloccare
        parker.park();
        parker.park();
        parker.park();
    }

    #[test]
    fn test_initial_token_consumed() {
        let (parker, _unparker) = ThreadParker::new();

        // Il token iniziale (UNPARKED = 1) dovrebbe permettere un park senza blocco
        let start = Instant::now();
        parker.park();
        let elapsed = start.elapsed();

        assert!(elapsed < Duration::from_millis(100));
    }

    // ==================== CLONE ====================

    #[test]
    fn test_parker_clone() {
        let (parker, unparker) = ThreadParker::new();
        let parker2 = parker.clone();

        unparker.unpark();
        unparker.unpark();

        // Entrambi i parker condividono i token
        parker.park();
        parker2.park();
    }

    #[test]
    fn test_unparker_clone() {
        let (parker, unparker) = ThreadParker::new();
        let unparker2 = unparker.clone();

        // Entrambi gli unparker aggiungono token allo stesso pool
        unparker.unpark();
        unparker2.unpark();

        parker.park();
        parker.park();
    }

    #[test]
    fn test_drop_partial_clones() {
        let (parker, unparker) = ThreadParker::new();
        let p2 = parker.clone();
        let u2 = unparker.clone();

        drop(p2);
        drop(u2);

        // Originali ancora funzionanti
        unparker.unpark();
        parker.park();
    }

    // ==================== CONCURRENT - BASIC ====================

    #[test]
    fn test_unpark_wakes_parked_thread() {
        let (parker, unparker) = ThreadParker::new();
        let woken = Arc::new(AtomicUsize::new(0));

        let w = woken.clone();
        let handle = thread::spawn(move || {
            parker.park(); // Consuma token iniziale
            parker.park(); // Questo dovrebbe bloccare
            w.store(1, Ordering::SeqCst);
        });

        // Dai tempo al thread di bloccarsi
        thread::sleep(Duration::from_millis(50));
        assert_eq!(woken.load(Ordering::SeqCst), 0, "Thread woke up too early");

        // Unpark dovrebbe svegliare il thread
        unparker.unpark();

        handle.join().unwrap();
        assert_eq!(woken.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_unpark_from_another_thread() {
        let (parker, unparker) = ThreadParker::new();
        let done = Arc::new(AtomicUsize::new(0));

        let d = done.clone();
        let park_thread = thread::spawn(move || {
            parker.park(); // Token iniziale
            parker.park(); // Blocca
            d.store(1, Ordering::SeqCst);
        });

        thread::sleep(Duration::from_millis(30));

        let unpark_thread = thread::spawn(move || {
            unparker.unpark();
        });

        unpark_thread.join().unwrap();
        park_thread.join().unwrap();

        assert_eq!(done.load(Ordering::SeqCst), 1);
    }

    // ==================== CONCURRENT - MULTIPLE THREADS ====================

    #[test]
    fn test_multiple_parkers_one_unparker() {
        let (parker, unparker) = ThreadParker::new();
        let counter = Arc::new(AtomicUsize::new(0));

        // Pre-unpark per i thread
        for _ in 0..4 {
            unparker.unpark();
        }

        let handles: Vec<_> = (0..4)
            .map(|_| {
                let p = parker.clone();
                let c = counter.clone();
                thread::spawn(move || {
                    p.park();
                    c.fetch_add(1, Ordering::SeqCst);
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(counter.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn test_multiple_unparkers_one_parker() {
        let (parker, unparker) = ThreadParker::new();
        let counter = Arc::new(AtomicUsize::new(0));

        // Più thread fanno unpark
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let u = unparker.clone();
                let c = counter.clone();
                thread::spawn(move || {
                    u.unpark();
                    c.fetch_add(1, Ordering::SeqCst);
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        // Dovremmo avere almeno 4 token extra
        parker.park();
        parker.park();
        parker.park();
        parker.park();

        assert_eq!(counter.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn test_concurrent_park_unpark() {
        let (parker, unparker) = ThreadParker::new();
        let completed = Arc::new(AtomicUsize::new(0));

        // Pre-unpark alcuni token
        unparker.unpark();
        unparker.unpark();

        // Thread che fanno park
        let park_handles: Vec<_> = (0..5)
            .map(|_| {
                let p = parker.clone();
                let c = completed.clone();
                thread::spawn(move || {
                    p.park();
                    c.fetch_add(1, Ordering::SeqCst);
                })
            })
            .collect();

        // Thread che fanno unpark (per sbloccare i park_handles)
        let unpark_handles: Vec<_> = (0..5)
            .map(|_| {
                let u = unparker.clone();
                thread::spawn(move || {
                    thread::sleep(Duration::from_millis(10));
                    u.unpark();
                })
            })
            .collect();

        for h in unpark_handles {
            h.join().unwrap();
        }

        for h in park_handles {
            h.join().unwrap();
        }

        assert_eq!(completed.load(Ordering::SeqCst), 5);
    }

    // ==================== STRESS ====================

    #[test]
    fn test_rapid_park_unpark() {
        let (parker, unparker) = ThreadParker::new();

        for _ in 0..100 {
            unparker.unpark();
            parker.park();
        }
    }

    #[test]
    fn test_producer_consumer_pattern() {
        let (parker, unparker) = ThreadParker::new();
        let produced = Arc::new(AtomicUsize::new(0));
        let consumed = Arc::new(AtomicUsize::new(0));

        let p = produced.clone();
        let producer = thread::spawn(move || {
            for _ in 0..20 {
                p.fetch_add(1, Ordering::SeqCst);
                unparker.unpark();
                thread::yield_now();
            }
        });

        let c = consumed.clone();
        let consumer = thread::spawn(move || {
            for _ in 0..20 {
                parker.park();
                c.fetch_add(1, Ordering::SeqCst);
            }
        });

        producer.join().unwrap();
        consumer.join().unwrap();

        assert_eq!(produced.load(Ordering::SeqCst), 20);
        assert_eq!(consumed.load(Ordering::SeqCst), 20);
    }

    #[test]
    fn test_many_threads_coordination() {
        let (parker, unparker) = ThreadParker::new();
        let ready = Arc::new(AtomicUsize::new(0));

        // Pre-unpark per tutti i thread
        for _ in 0..10 {
            unparker.unpark();
        }

        let handles: Vec<_> = (0..10)
            .map(|i| {
                let p = parker.clone();
                let u = unparker.clone();
                let r = ready.clone();
                thread::spawn(move || {
                    p.park();
                    r.fetch_add(1, Ordering::SeqCst);
                    // Ogni thread tranne l'ultimo fa unpark per il prossimo ciclo
                    if i < 9 {
                        u.unpark();
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(ready.load(Ordering::SeqCst), 10);
    }

    // ==================== EDGE CASES ====================

    #[test]
    fn test_unpark_many_then_park_many() {
        let (parker, unparker) = ThreadParker::new();

        // Accumula molti token
        for _ in 0..50 {
            unparker.unpark();
        }

        // Consuma tutti
        for _ in 0..50 {
            parker.park();
        }
    }

    #[test]
    fn test_interleaved_park_unpark() {
        let (parker, unparker) = ThreadParker::new();

        for i in 0..20 {
            if i % 2 == 0 {
                unparker.unpark();
                unparker.unpark();
            }
            parker.park();
        }
    }

    #[test]
    fn test_drop_while_parked() {
        let (parker, unparker) = ThreadParker::new();
        let dropped = Arc::new(AtomicUsize::new(0));

        let d = dropped.clone();
        let handle = thread::spawn(move || {
            parker.park(); // Token iniziale
            parker.park(); // Questo bloccherà
            d.store(1, Ordering::SeqCst);
        });

        thread::sleep(Duration::from_millis(30));

        // Unpark per sbloccare prima di droppare
        unparker.unpark();

        handle.join().unwrap();
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_clone_drop_stress() {
        let (parker, unparker) = ThreadParker::new();

        let handles: Vec<_> = (0..10)
            .map(|_| {
                let p = parker.clone();
                let u = unparker.clone();
                thread::spawn(move || {
                    u.unpark();
                    p.park();
                    drop(p);
                    drop(u);
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        // Originali ancora validi
        unparker.unpark();
        parker.park();
    }
}