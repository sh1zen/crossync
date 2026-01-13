#[cfg(test)]
mod tests_watch_guard_ref {
    use crate::sync::{RwLock, WatchGuardRef};
    use std::any::Any;
    use std::sync::{Arc, Barrier};
    use std::thread;

    // ==================== BASIC FUNCTIONALITY ====================

    #[test]
    fn test_deref() {
        let lock = RwLock::new(42i32);
        let guard = lock.lock_shared();

        assert_eq!(*guard, 42);
    }

    #[test]
    fn test_deref_complex_type() {
        let lock = RwLock::new(vec![1, 2, 3]);
        let guard = lock.lock_shared();

        assert_eq!(guard.len(), 3);
        assert_eq!(guard[0], 1);
        assert_eq!(guard.first(), Some(&1));
    }

    #[test]
    fn test_deref_string() {
        let lock = RwLock::new(String::from("hello world"));
        let guard = lock.lock_shared();

        assert!(guard.starts_with("hello"));
        assert!(guard.contains("world"));
        assert_eq!(guard.len(), 11);
    }

    // ==================== LOCK STATE ====================

    #[test]
    fn test_is_locked() {
        let lock = RwLock::new(0);

        let guard = lock.lock_shared();
        assert!(guard.is_locked());

        drop(guard);
    }

    #[test]
    fn test_drop_releases_lock() {
        let lock = RwLock::new(0);

        {
            let _guard = lock.lock_shared();
        }

        // Deve poter acquisire exclusive dopo shared rilasciato
        let _guard2 = lock.lock_exclusive();
    }

    #[test]
    fn test_multiple_shared_guards() {
        let lock = RwLock::new(42);

        let g1 = lock.lock_shared();
        let g2 = lock.lock_shared();
        let g3 = lock.lock_shared();

        assert_eq!(*g1, 42);
        assert_eq!(*g2, 42);
        assert_eq!(*g3, 42);
    }

    // ==================== PARTIAL_EQ ====================

    #[test]
    fn test_partial_eq() {
        let lock = RwLock::new(42i32);
        let guard = lock.lock_shared();

        assert!(guard == 42);
        assert!(guard != 0);
    }

    #[test]
    fn test_partial_eq_string() {
        let lock = RwLock::new(String::from("channels"));
        let guard = lock.lock_shared();

        assert!(guard == "channels");
        assert!(guard != "other");
    }

    #[test]
    fn test_partial_eq_vec() {
        let lock = RwLock::new(vec![1, 2, 3]);
        let guard = lock.lock_shared();

        assert!(guard == vec![1, 2, 3]);
        assert!(guard == [1, 2, 3].as_slice());
        assert!(guard != vec![1, 2]);
    }

    // ==================== DEBUG ====================

    #[test]
    fn test_debug() {
        let lock = RwLock::new(42);
        let guard = lock.lock_shared();

        let debug_str = format!("{:?}", guard);
        assert!(debug_str.contains("WatchGuardRef"));
        assert!(debug_str.contains("data"));
        assert!(debug_str.contains("lock"));
    }

    // ==================== DOWNCAST ====================

    #[test]
    fn test_downcast_success() {
        let lock = RwLock::new(Box::new(String::from("hello")) as Box<dyn Any>);

        let guard = lock.lock_shared();
        unsafe {
            let str_guard = WatchGuardRef::downcast::<String>(guard).unwrap();
            assert_eq!(*str_guard, "hello");
        }
    }

    #[test]
    fn test_downcast_read_only() {
        let lock = RwLock::new(Box::new(vec![1, 2, 3]) as Box<dyn Any>);

        let guard = lock.lock_shared();
        unsafe {
            let vec_guard = WatchGuardRef::downcast::<Vec<i32>>(guard).unwrap();
            assert_eq!(vec_guard.len(), 3);
            assert_eq!(vec_guard[0], 1);
            assert_eq!(vec_guard.iter().sum::<i32>(), 6);
        }
    }

    #[test]
    fn test_downcast_wrong_type() {
        let lock = RwLock::new(Box::new(42i32) as Box<dyn Any>);

        let guard = lock.lock_shared();
        unsafe {
            let result = WatchGuardRef::downcast::<String>(guard);
            assert!(result.is_err());

            // Il guard originale viene restituito
            let original = result.unwrap_err();
            assert!(original.is_locked());
        }
    }

    #[test]
    fn test_downcast_multiple_readers() {
        let lock = RwLock::new(Box::new(String::from("shared")) as Box<dyn Any>);

        let g1 = lock.lock_shared();
        let g2 = lock.lock_shared();

        unsafe {
            let str_g1 = WatchGuardRef::downcast::<String>(g1).unwrap();
            let str_g2 = WatchGuardRef::downcast::<String>(g2).unwrap();

            assert_eq!(*str_g1, "shared");
            assert_eq!(*str_g2, "shared");
        }
    }

    #[test]
    fn test_downcast_various_types() {
        // i32
        let lock_i32 = RwLock::new(Box::new(42i32) as Box<dyn Any>);
        let guard = lock_i32.lock_shared();
        unsafe {
            let g = WatchGuardRef::downcast::<i32>(guard).unwrap();
            assert_eq!(*g, 42);
        }

        // f64
        let lock_f64 = RwLock::new(Box::new(3.14f64) as Box<dyn Any>);
        let guard = lock_f64.lock_shared();
        unsafe {
            let g = WatchGuardRef::downcast::<f64>(guard).unwrap();
            assert!((*g - 3.14).abs() < 0.001);
        }

        // bool
        let lock_bool = RwLock::new(Box::new(true) as Box<dyn Any>);
        let guard = lock_bool.lock_shared();
        unsafe {
            let g = WatchGuardRef::downcast::<bool>(guard).unwrap();
            assert!(*g);
        }

        // tuple
        let lock_tuple = RwLock::new(Box::new((1, "hello", 3.0)) as Box<dyn Any>);
        let guard = lock_tuple.lock_shared();
        unsafe {
            let g = WatchGuardRef::downcast::<(i32, &str, f64)>(guard).unwrap();
            assert_eq!(g.0, 1);
            assert_eq!(g.1, "hello");
        }
    }

    // ==================== CONCURRENT ====================

    #[test]
    fn test_concurrent_shared_readers() {
        let lock = Arc::new(RwLock::new(42i64));
        let barrier = Arc::new(Barrier::new(8));
        let sum = Arc::new(std::sync::atomic::AtomicI64::new(0));

        let handles: Vec<_> = (0..8)
            .map(|_| {
                let l = lock.clone();
                let b = barrier.clone();
                let s = sum.clone();
                thread::spawn(move || {
                    b.wait();
                    for _ in 0..500 {
                        let guard = l.lock_shared();
                        s.fetch_add(*guard, std::sync::atomic::Ordering::Relaxed);
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(sum.load(std::sync::atomic::Ordering::SeqCst), 42 * 8 * 500);
    }

    #[test]
    fn test_concurrent_downcast_readers() {
        let lock = Arc::new(RwLock::new(Box::new(100i64) as Box<dyn Any + Send + Sync>));
        let barrier = Arc::new(Barrier::new(4));
        let sum = Arc::new(std::sync::atomic::AtomicI64::new(0));

        let handles: Vec<_> = (0..4)
            .map(|_| {
                let l = lock.clone();
                let b = barrier.clone();
                let s = sum.clone();
                thread::spawn(move || {
                    b.wait();
                    for _ in 0..200 {
                        let guard = l.lock_shared();
                        if let Some(val) = (**guard).downcast_ref::<i64>() {
                            s.fetch_add(*val, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(sum.load(std::sync::atomic::Ordering::SeqCst), 100 * 4 * 200);
    }

    #[test]
    fn test_concurrent_readers_with_writer() {
        let lock = Arc::new(RwLock::new(0i64));
        let barrier = Arc::new(Barrier::new(5));

        // 4 readers
        let readers: Vec<_> = (0..4)
            .map(|_| {
                let l = lock.clone();
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                    for _ in 0..500 {
                        let guard = l.lock_shared();
                        let _ = *guard; // Just read
                    }
                })
            })
            .collect();

        // 1 writer
        let l = lock.clone();
        let b = barrier.clone();
        let writer = thread::spawn(move || {
            b.wait();
            for i in 0..100 {
                let mut guard = l.lock_exclusive();
                *guard = i;
            }
        });

        for r in readers {
            r.join().unwrap();
        }
        writer.join().unwrap();

        assert_eq!(*lock.lock_shared(), 99);
    }

    // ==================== EDGE CASES ====================

    #[test]
    fn test_zero_sized_type() {
        let lock = RwLock::new(());

        let g1 = lock.lock_shared();
        let g2 = lock.lock_shared();

        drop(g1);
        drop(g2);
    }

    #[test]
    fn test_nested_reference() {
        let inner = Arc::new(42);
        let lock = RwLock::new(inner.clone());

        let guard = lock.lock_shared();
        assert_eq!(**guard, 42);
    }

    #[test]
    fn test_option_value() {
        let lock = RwLock::new(Some(42i32));

        let guard = lock.lock_shared();
        assert_eq!(*guard, Some(42));
        assert!(guard.is_some());
        assert_eq!(guard.unwrap(), 42);
    }

    #[test]
    fn test_result_value() {
        let lock = RwLock::new(Ok::<i32, &str>(42));

        let guard = lock.lock_shared();
        assert!(guard.is_ok());
        assert_eq!(*guard.as_ref().unwrap(), 42);
    }

    #[test]
    fn test_slice_methods() {
        let lock = RwLock::new(vec![5, 3, 8, 1, 9]);

        let guard = lock.lock_shared();
        assert_eq!(guard.iter().max(), Some(&9));
        assert_eq!(guard.iter().min(), Some(&1));
        assert_eq!(guard.iter().sum::<i32>(), 26);
        assert!(guard.contains(&8));
        assert!(!guard.contains(&100));
    }

    // ==================== GUARD SEMANTICS ====================

    #[test]
    fn test_guard_lifetime() {
        let lock = RwLock::new(String::from("channels"));

        let len = {
            let guard = lock.lock_shared();
            guard.len()
        };

        assert_eq!(len, 8);
    }

    #[test]
    fn test_sequential_guards() {
        let lock = RwLock::new(42);

        for _ in 0..100 {
            let guard = lock.lock_shared();
            assert_eq!(*guard, 42);
        }
    }

    #[test]
    fn test_guard_with_iterator() {
        let lock = RwLock::new(vec![1, 2, 3, 4, 5]);

        let guard = lock.lock_shared();
        let doubled: Vec<i32> = guard.iter().map(|x| x * 2).collect();

        assert_eq!(doubled, vec![2, 4, 6, 8, 10]);
    }

    #[test]
    fn test_guard_with_hashmap() {
        use std::collections::HashMap;

        let mut map = HashMap::new();
        map.insert("a", 1);
        map.insert("b", 2);
        map.insert("c", 3);

        let lock = RwLock::new(map);
        let guard = lock.lock_shared();

        assert_eq!(guard.get("a"), Some(&1));
        assert_eq!(guard.get("b"), Some(&2));
        assert!(guard.contains_key("c"));
        assert!(!guard.contains_key("d"));
        assert_eq!(guard.len(), 3);
    }
}
