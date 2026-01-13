use std::sync::atomic::AtomicUsize;

pub(crate) type Futex = AtomicUsize;

#[cfg(any(target_os = "linux", target_os = "android"))]
#[path = "linux.rs"]
mod platform;

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "watchos"))]
#[path = "macos.rs"]
mod platform;

#[cfg(windows)]
#[path = "windows.rs"]
mod platform;

#[cfg(target_os = "freebsd")]
#[path = "freebsd.rs"]
mod platform;

/// If atomic and value matches, wait until woken up.
/// This function might also return spuriously. Handle that case.
#[inline]
pub(crate) fn futex_wait(atomic: &AtomicUsize, value: usize) {
    platform::wait(atomic, value)
}

/// Wake one thread that is waiting on this atomic.
/// It's okay if the pointer dangles or is null.
#[inline]
pub(crate) fn futex_wake(atomic: *const AtomicUsize) {
    platform::wake_one(atomic);
}

/// Wake all threads that are waiting on this atomic.
/// It's okay if the pointer dangles or is null.
#[inline]
pub(crate) fn futex_wake_all(atomic: *const AtomicUsize) {
    platform::wake_all(atomic);
}

#[cfg(test)]
mod tests_futex {
    use crate::core::futex::{futex_wait, futex_wake, futex_wake_all};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::{Duration, Instant};

    // ==================== BASIC WAKE (NO WAITERS) ====================

    #[test]
    fn test_wake_null_pointer() {
        // Non deve crashare con puntatore nullo
        futex_wake(std::ptr::null::<AtomicUsize>());
        futex_wake_all(std::ptr::null::<AtomicUsize>());
    }

    #[test]
    fn test_wake_no_waiters() {
        let a = AtomicUsize::new(0);

        // Wake senza thread in attesa - non deve bloccare o crashare
        futex_wake(&a);
        futex_wake_all(&a);
    }

    #[test]
    fn test_wake_dangling_safe() {
        // Simula puntatore "dangling" (non nullo ma non valido)
        // In pratica usiamo un indirizzo stack che non ha waiters
        let a = AtomicUsize::new(42);
        let ptr = &a as *const AtomicUsize;

        futex_wake(ptr);
    }

    // ==================== WAIT BEHAVIOR ====================

    #[test]
    fn test_wait_value_mismatch_returns_immediately() {
        let a = AtomicUsize::new(0);

        let start = Instant::now();
        // Valore non corrisponde -> ritorna subito
        futex_wait(&a, 1);
        let elapsed = start.elapsed();

        assert!(
            elapsed < Duration::from_millis(50),
            "wait blocked unexpectedly"
        );
    }

    #[test]
    fn test_wait_value_mismatch_various() {
        let a = AtomicUsize::new(100);

        for wrong_value in [0, 1, 50, 99, 101, usize::MAX] {
            let start = Instant::now();
            futex_wait(&a, wrong_value);
            assert!(start.elapsed() < Duration::from_millis(50));
        }
    }

    // ==================== WAIT + WAKE ====================

    #[test]
    fn test_wake_one_wakes_waiter() {
        let a = Arc::new(AtomicUsize::new(0));
        let woken = Arc::new(AtomicUsize::new(0));

        let a2 = a.clone();
        let w = woken.clone();
        let handle = thread::spawn(move || {
            while a2.load(Ordering::Acquire) == 0 {
                futex_wait(&a2, 0);
            }
            w.store(1, Ordering::Release);
        });

        thread::sleep(Duration::from_millis(50));
        assert_eq!(woken.load(Ordering::Acquire), 0);

        a.store(1, Ordering::Release);
        futex_wake(&*a);

        handle.join().unwrap();
        assert_eq!(woken.load(Ordering::Acquire), 1);
    }

    #[test]
    fn test_wake_all_wakes_multiple() {
        let a = Arc::new(AtomicUsize::new(0));
        let woken_count = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(5)); // 4 waiters + 1 waker

        let handles: Vec<_> = (0..4)
            .map(|_| {
                let a = a.clone();
                let w = woken_count.clone();
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait(); // Sincronizza inizio
                    while a.load(Ordering::Acquire) == 0 {
                        futex_wait(&a, 0);
                    }
                    w.fetch_add(1, Ordering::SeqCst);
                })
            })
            .collect();

        barrier.wait();
        thread::sleep(Duration::from_millis(50));

        a.store(1, Ordering::Release);
        futex_wake_all(&*a);

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(woken_count.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn test_wake_one_wakes_only_one() {
        let a = Arc::new(AtomicUsize::new(0));
        let woken_count = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(4)); // 3 waiters + 1 controller

        let handles: Vec<_> = (0..3)
            .map(|_| {
                let a = a.clone();
                let w = woken_count.clone();
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                    // Aspetta finché il valore diventa il nostro "ticket"
                    loop {
                        let val = a.load(Ordering::Acquire);
                        if val > 0 {
                            w.fetch_add(1, Ordering::SeqCst);
                            break;
                        }
                        futex_wait(&a, 0);
                    }
                })
            })
            .collect();

        barrier.wait();
        thread::sleep(Duration::from_millis(50));

        // Wake uno alla volta
        for i in 1..=3 {
            a.store(i, Ordering::Release);
            futex_wake(&*a);
            thread::sleep(Duration::from_millis(30));
        }

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(woken_count.load(Ordering::SeqCst), 3);
    }

    // ==================== TIMING ====================

    #[test]
    fn test_wait_wake_timing() {
        let a = Arc::new(AtomicUsize::new(0));

        let a2 = a.clone();
        let start = Instant::now();

        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            a2.store(1, Ordering::Release);
            futex_wake(&*a2);
        });

        while a.load(Ordering::Acquire) == 0 {
            futex_wait(&a, 0);
        }

        let elapsed = start.elapsed();
        handle.join().unwrap();

        // Dovrebbe essere circa 100ms (con margine per scheduling)
        assert!(elapsed >= Duration::from_millis(80));
        assert!(elapsed < Duration::from_millis(500));
    }

    // ==================== CONCURRENT PATTERNS ====================

    #[test]
    fn test_producer_consumer_pattern() {
        let state = Arc::new(AtomicUsize::new(0));
        let consumed = Arc::new(AtomicUsize::new(0));

        let s = state.clone();
        let c = consumed.clone();
        let consumer = thread::spawn(move || {
            for expected in 1..=10 {
                while s.load(Ordering::Acquire) < expected {
                    futex_wait(&s, expected - 1);
                }
                c.fetch_add(1, Ordering::SeqCst);
            }
        });

        let s = state.clone();
        let producer = thread::spawn(move || {
            for i in 1..=10 {
                s.store(i, Ordering::Release);
                futex_wake(&*s);
                thread::yield_now();
            }
        });

        producer.join().unwrap();
        consumer.join().unwrap();

        assert_eq!(consumed.load(Ordering::SeqCst), 10);
    }

    #[test]
    fn test_flag_pattern() {
        let flag = Arc::new(AtomicUsize::new(0));
        let completed = Arc::new(AtomicUsize::new(0));

        // Thread che aspetta il flag
        let f = flag.clone();
        let c = completed.clone();
        let waiter = thread::spawn(move || {
            while f.load(Ordering::Acquire) == 0 {
                futex_wait(&f, 0);
            }
            c.store(1, Ordering::Release);
        });

        thread::sleep(Duration::from_millis(50));
        assert_eq!(completed.load(Ordering::Acquire), 0);

        // Setta il flag e sveglia
        flag.store(1, Ordering::Release);
        futex_wake(&*flag);

        waiter.join().unwrap();
        assert_eq!(completed.load(Ordering::Acquire), 1);
    }

    #[test]
    fn test_counter_pattern() {
        let counter = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(5));

        let handles: Vec<_> = (0..4)
            .map(|_| {
                let c = counter.clone();
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                    for _ in 0..10 {
                        c.fetch_add(1, Ordering::SeqCst);
                        futex_wake_all(&*c);
                        thread::yield_now();
                    }
                })
            })
            .collect();

        barrier.wait();

        for h in handles {
            h.join().unwrap();
        }

        // 4 threads * 10 increments = 40
        assert_eq!(counter.load(Ordering::SeqCst), 40);
    }

    // ==================== EDGE CASES ====================

    #[test]
    fn test_rapid_wait_wake() {
        let a = Arc::new(AtomicUsize::new(0));

        for i in 0..50 {
            let a2 = a.clone();
            let handle = thread::spawn(move || {
                while a2.load(Ordering::Acquire) != i + 1 {
                    futex_wait(&a2, i);
                }
            });

            a.store(i + 1, Ordering::Release);
            futex_wake(&*a);

            handle.join().unwrap();
        }
    }

    #[test]
    fn test_multiple_atomics() {
        let a1 = Arc::new(AtomicUsize::new(0));
        let a2 = Arc::new(AtomicUsize::new(0));

        let a1c = a1.clone();
        let a2c = a2.clone();

        let h1 = thread::spawn(move || {
            while a1c.load(Ordering::Acquire) == 0 {
                futex_wait(&a1c, 0);
            }
        });

        let h2 = thread::spawn(move || {
            while a2c.load(Ordering::Acquire) == 0 {
                futex_wait(&a2c, 0);
            }
        });

        thread::sleep(Duration::from_millis(50));

        // Wake separatamente
        a1.store(1, Ordering::Release);
        futex_wake(&*a1);

        a2.store(1, Ordering::Release);
        futex_wake(&*a2);

        h1.join().unwrap();
        h2.join().unwrap();
    }

    #[test]
    fn test_value_boundaries() {
        let a = AtomicUsize::new(usize::MAX);

        // Wait con valore che non corrisponde
        let start = Instant::now();
        futex_wait(&a, 0);
        futex_wait(&a, usize::MAX - 1);
        assert!(start.elapsed() < Duration::from_millis(50));

        // Wait con valore che corrisponde (potrebbe bloccare brevemente o spurious return)
        // Non testiamo il blocking perché potrebbe non ritornare
    }

    #[test]
    fn test_zero_value() {
        let a = AtomicUsize::new(0);

        // Wake su valore 0
        futex_wake(&a);
        futex_wake_all(&a);

        // Wait con valore 0 che non corrisponde (atomic è 0, aspettiamo 1)
        let start = Instant::now();
        futex_wait(&a, 1);
        assert!(start.elapsed() < Duration::from_millis(50));
    }
}
