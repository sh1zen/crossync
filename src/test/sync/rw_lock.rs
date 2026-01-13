#[cfg(test)]
mod tests_rwlock {
    use crate::sync::RwLock;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};
    use std::thread;
    // ==================== CONSTRUCTION ====================

    #[test]
    fn test_new() {
        let lock = RwLock::new(42);
        assert_eq!(*lock.lock_shared(), 42);
        assert!(!lock.is_locked());
    }

    #[test]
    fn test_default() {
        let lock: RwLock<i32> = RwLock::default();
        assert_eq!(*lock.lock_shared(), 0);

        let lock: RwLock<String> = RwLock::default();
        assert_eq!(*lock.lock_shared(), "");

        let lock: RwLock<Vec<i32>> = RwLock::default();
        assert!(lock.lock_shared().is_empty());
    }

    #[test]
    fn test_debug() {
        let lock = RwLock::new(42);
        let debug_str = format!("{:?}", lock);
        assert!(debug_str.contains("MutexCell"));
    }

    // ==================== EXCLUSIVE LOCKING ====================

    #[test]
    fn test_lock_exclusive() {
        let lock = RwLock::new(10);

        {
            let mut guard = lock.lock_exclusive();
            assert_eq!(*guard, 10);
            *guard = 20;
        }

        assert_eq!(*lock.lock_shared(), 20);
    }

    #[test]
    fn test_lock_exclusive_sequential() {
        let lock = RwLock::new(0);

        for i in 1..=10 {
            let mut guard = lock.lock_exclusive();
            *guard += i;
        }

        assert_eq!(*lock.lock_shared(), 55); // 1+2+...+10
    }

    #[test]
    fn test_try_lock_success() {
        let lock = RwLock::new(42);

        let guard = lock.try_lock();
        assert!(guard.is_some());
        assert_eq!(*guard.unwrap(), 42);
    }

    #[test]
    fn test_try_lock_fail_when_exclusive() {
        let lock = RwLock::new(42);

        let _guard = lock.lock_exclusive();
        assert!(lock.try_lock().is_none());
    }

    #[test]
    fn test_try_lock_fail_when_shared() {
        let lock = RwLock::new(42);

        let _guard = lock.lock_shared();
        // try_lock (exclusive) dovrebbe fallire con shared attivo
        assert!(lock.try_lock().is_none());
    }

    // ==================== SHARED LOCKING ====================

    #[test]
    fn test_lock_shared() {
        let lock = RwLock::new(42);

        let guard = lock.lock_shared();
        assert_eq!(*guard, 42);
    }

    #[test]
    fn test_lock_shared_multiple() {
        let lock = RwLock::new(100);

        let g1 = lock.lock_shared();
        let g2 = lock.lock_shared();
        let g3 = lock.lock_shared();

        assert_eq!(*g1, 100);
        assert_eq!(*g2, 100);
        assert_eq!(*g3, 100);
    }

    #[test]
    fn test_try_lock_shared_success() {
        let lock = RwLock::new(42);

        let guard = lock.try_lock_shared();
        assert!(guard.is_some());
        assert_eq!(*guard.unwrap(), 42);
    }

    #[test]
    fn test_try_lock_shared_multiple() {
        let lock = RwLock::new(42);

        let g1 = lock.try_lock_shared();
        let g2 = lock.try_lock_shared();

        assert!(g1.is_some());
        assert!(g2.is_some());
    }

    #[test]
    fn test_try_lock_shared_fail_when_exclusive() {
        let lock = RwLock::new(42);

        let _guard = lock.lock_exclusive();
        assert!(lock.try_lock_shared().is_none());
    }

    // ==================== LOCK STATE ====================

    #[test]
    fn test_is_locked() {
        let lock = RwLock::new(0);

        assert!(!lock.is_locked());

        {
            let _g = lock.lock_exclusive();
            assert!(lock.is_locked());
        }

        assert!(!lock.is_locked());

        {
            let _g = lock.lock_shared();
            assert!(lock.is_locked());
        }

        assert!(!lock.is_locked());
    }

    #[test]
    fn test_is_locked_exclusive() {
        let lock = RwLock::new(0);

        assert!(!lock.is_locked_exclusive());

        {
            let _g = lock.lock_exclusive();
            assert!(lock.is_locked_exclusive());
        }

        assert!(!lock.is_locked_exclusive());

        {
            let _g = lock.lock_shared();
            assert!(!lock.is_locked_exclusive());
        }
    }

    #[test]
    fn test_is_locked_shared() {
        let lock = RwLock::new(0);

        assert!(!lock.is_locked_shared());

        {
            let _g = lock.lock_shared();
            assert!(lock.is_locked_shared());
        }

        assert!(!lock.is_locked_shared());

        {
            let _g = lock.lock_exclusive();
            assert!(!lock.is_locked_shared());
        }
    }

    // ==================== CLONE & REFCOUNT ====================

    #[test]
    fn test_clone_shares_data() {
        let lock = RwLock::new(10);
        let clone = lock.clone();

        {
            let mut guard = lock.lock_exclusive();
            *guard = 99;
        }

        assert_eq!(*clone.lock_shared(), 99);
    }

    #[test]
    fn test_clone_multiple() {
        let lock = RwLock::new(0);
        let c1 = lock.clone();
        let c2 = lock.clone();
        let c3 = c1.clone();

        {
            let mut guard = c2.lock_exclusive();
            *guard = 42;
        }

        assert_eq!(*lock.lock_shared(), 42);
        assert_eq!(*c1.lock_shared(), 42);
        assert_eq!(*c3.lock_shared(), 42);
    }

    #[test]
    fn test_clone_drop_partial() {
        let lock = RwLock::new(100);
        let c1 = lock.clone();
        let c2 = lock.clone();

        drop(c1);
        drop(c2);

        // Originale ancora valido
        assert_eq!(*lock.lock_shared(), 100);
    }

    // ==================== MEMORY SAFETY ====================

    #[derive(Clone)]
    struct DropTracker {
        counter: Arc<AtomicUsize>,
    }

    impl DropTracker {
        fn new(counter: Arc<AtomicUsize>) -> Self {
            counter.fetch_add(1, Ordering::SeqCst);
            Self { counter }
        }
    }

    impl Drop for DropTracker {
        fn drop(&mut self) {
            self.counter.fetch_sub(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn test_no_leak_on_drop() {
        let counter = Arc::new(AtomicUsize::new(0));

        {
            let lock = RwLock::new(DropTracker::new(counter.clone()));
            assert_eq!(counter.load(Ordering::SeqCst), 1);
            drop(lock);
        }

        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_no_leak_with_clones() {
        let counter = Arc::new(AtomicUsize::new(0));

        {
            let lock = RwLock::new(DropTracker::new(counter.clone()));
            let c1 = lock.clone();
            let c2 = lock.clone();

            assert_eq!(counter.load(Ordering::SeqCst), 1);

            drop(c1);
            drop(c2);

            assert_eq!(counter.load(Ordering::SeqCst), 1);
        }

        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    // ==================== CONCURRENT ====================

    #[test]
    fn test_concurrent_exclusive() {
        let lock = Arc::new(RwLock::new(0i64));
        let barrier = Arc::new(Barrier::new(4));

        let handles: Vec<_> = (0..4)
            .map(|_| {
                let l = lock.clone();
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                    for _ in 0..500 {
                        let mut guard = l.lock_exclusive();
                        *guard += 1;
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(*lock.lock_shared(), 2000);
    }

    #[test]
    fn test_concurrent_shared() {
        let lock = Arc::new(RwLock::new(42i64));
        let barrier = Arc::new(Barrier::new(8));
        let sum = Arc::new(AtomicUsize::new(0));

        let handles: Vec<_> = (0..8)
            .map(|_| {
                let l = lock.clone();
                let b = barrier.clone();
                let s = sum.clone();
                thread::spawn(move || {
                    b.wait();
                    for _ in 0..500 {
                        let guard = l.lock_shared();
                        s.fetch_add(*guard as usize, Ordering::Relaxed);
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(sum.load(Ordering::SeqCst), 42 * 8 * 500);
    }

    #[test]
    fn test_concurrent_mixed() {
        let lock = Arc::new(RwLock::new(0i64));
        let barrier = Arc::new(Barrier::new(8));

        // 4 writers
        let writers: Vec<_> = (0..4)
            .map(|_| {
                let l = lock.clone();
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                    for _ in 0..200 {
                        let mut guard = l.lock_exclusive();
                        *guard += 1;
                    }
                })
            })
            .collect();

        // 4 readers
        let readers: Vec<_> = (0..4)
            .map(|_| {
                let l = lock.clone();
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                    for _ in 0..500 {
                        let _guard = l.lock_shared();
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

        assert_eq!(*lock.lock_shared(), 800);
    }

    #[test]
    fn test_concurrent_clone_drop() {
        let counter = Arc::new(AtomicUsize::new(0));
        let lock = Arc::new(RwLock::new(DropTracker::new(counter.clone())));
        let barrier = Arc::new(Barrier::new(10));

        let handles: Vec<_> = (0..10)
            .map(|_| {
                let l = (*lock).clone();
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                    thread::yield_now();
                    drop(l);
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }
        drop(lock);

        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_concurrent_try_lock() {
        let lock = Arc::new(RwLock::new(0i64));
        let barrier = Arc::new(Barrier::new(8));
        let success_count = Arc::new(AtomicUsize::new(0));

        let handles: Vec<_> = (0..8)
            .map(|_| {
                let l = lock.clone();
                let b = barrier.clone();
                let sc = success_count.clone();
                thread::spawn(move || {
                    b.wait();
                    for _ in 0..100 {
                        if let Some(mut guard) = l.try_lock() {
                            *guard += 1;
                            sc.fetch_add(1, Ordering::Relaxed);
                        }
                        thread::yield_now();
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        let final_val = *lock.lock_shared();
        let successes = success_count.load(Ordering::SeqCst);
        assert_eq!(final_val as usize, successes);
    }

    // ==================== EDGE CASES ====================

    #[test]
    fn test_zero_sized_type() {
        let lock = RwLock::new(());

        {
            let _g = lock.lock_exclusive();
        }

        {
            let _g1 = lock.lock_shared();
            let _g2 = lock.lock_shared();
        }
    }

    #[test]
    fn test_large_value() {
        let large_data = vec![0u8; 1024 * 1024]; // 1MB
        let lock = RwLock::new(large_data);

        {
            let mut guard = lock.lock_exclusive();
            guard[0] = 42;
        }

        assert_eq!(lock.lock_shared()[0], 42);
    }

    #[test]
    fn test_complex_type() {
        use std::collections::HashMap;

        let lock = RwLock::new(HashMap::<String, Vec<i32>>::new());

        {
            let mut guard = lock.lock_exclusive();
            guard.insert("a".to_string(), vec![1, 2, 3]);
            guard.insert("b".to_string(), vec![4, 5]);
        }

        {
            let guard = lock.lock_shared();
            assert_eq!(guard.get("a"), Some(&vec![1, 2, 3]));
            assert_eq!(guard.len(), 2);
        }
    }

    #[test]
    fn test_option_value() {
        let lock = RwLock::new(None::<i32>);

        {
            let mut guard = lock.lock_exclusive();
            *guard = Some(42);
        }

        assert_eq!(*lock.lock_shared(), Some(42));
    }

    #[test]
    fn test_nested_rwlock() {
        let inner = Arc::new(RwLock::new(0));
        let outer = RwLock::new(inner.clone());

        {
            let outer_guard = outer.lock_shared();
            let mut inner_guard = outer_guard.lock_exclusive();
            *inner_guard = 42;
        }

        assert_eq!(*inner.lock_shared(), 42);
    }

    // ==================== GUARD BEHAVIOR ====================

    #[test]
    fn test_guard_drop_releases_lock() {
        let lock = RwLock::new(0);

        {
            let _guard = lock.lock_exclusive();
            assert!(lock.is_locked_exclusive());
        }

        assert!(!lock.is_locked());

        {
            let _guard = lock.lock_shared();
            assert!(lock.is_locked_shared());
        }

        assert!(!lock.is_locked());
    }

    #[test]
    fn test_exclusive_blocks_shared() {
        let lock = Arc::new(RwLock::new(0));
        let acquired = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(2));

        let l = lock.clone();
        let a = acquired.clone();
        let b = barrier.clone();

        // Thread che tiene exclusive lock
        let holder = thread::spawn(move || {
            let _guard = l.lock_exclusive();
            b.wait(); // Segnala che ha il lock
            thread::sleep(std::time::Duration::from_millis(50));
            a.store(1, Ordering::SeqCst);
        });

        barrier.wait(); // Aspetta che holder abbia il lock

        // Questo deve aspettare che holder rilasci
        let _guard = lock.lock_shared();
        assert_eq!(acquired.load(Ordering::SeqCst), 1);

        holder.join().unwrap();
    }
}
