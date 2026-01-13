#[cfg(test)]
mod tests_watch_guard_mut {
    use crate::sync::{RwLock, WatchGuardMut};
    use std::any::Any;
    use std::sync::{Arc, Barrier};
    use std::thread;

    // ==================== BASIC FUNCTIONALITY ====================

    #[test]
    fn test_deref() {
        let lock = RwLock::new(42i32);
        let guard = lock.lock_exclusive();

        assert_eq!(*guard, 42);
    }

    #[test]
    fn test_deref_mut() {
        let lock = RwLock::new(10i32);

        {
            let mut guard = lock.lock_exclusive();
            *guard = 20;
        }

        assert_eq!(*lock.lock_exclusive(), 20);
    }

    #[test]
    fn test_deref_complex_type() {
        let lock = RwLock::new(vec![1, 2, 3]);

        {
            let mut guard = lock.lock_exclusive();
            guard.push(4);
            assert_eq!(guard.len(), 4);
            assert_eq!(guard[0], 1);
        }

        assert_eq!(*lock.lock_exclusive(), vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_deref_string() {
        let lock = RwLock::new(String::from("hello"));

        {
            let mut guard = lock.lock_exclusive();
            guard.push_str(" world");
            assert!(guard.starts_with("hello"));
            assert_eq!(guard.len(), 11);
        }

        assert_eq!(*lock.lock_exclusive(), "hello world");
    }

    // ==================== LOCK STATE ====================

    #[test]
    fn test_is_locked() {
        let lock = RwLock::new(0);

        let guard = lock.lock_exclusive();
        assert!(guard.is_locked());

        drop(guard);
        // Dopo drop, il lock è rilasciato
    }

    #[test]
    fn test_drop_releases_lock() {
        let lock = RwLock::new(0);

        {
            let _guard = lock.lock_exclusive();
            // Lock acquisito
        }

        // Deve poter acquisire di nuovo
        let _guard2 = lock.lock_exclusive();
    }

    // ==================== PARTIAL_EQ ====================

    #[test]
    fn test_partial_eq() {
        let lock = RwLock::new(42i32);
        let guard = lock.lock_exclusive();

        assert!(guard == 42);
        assert!(guard != 0);
    }

    #[test]
    fn test_partial_eq_string() {
        let lock = RwLock::new(String::from("channels"));
        let guard = lock.lock_exclusive();

        assert!(guard == "channels");
        assert!(guard != "other");
    }

    #[test]
    fn test_partial_eq_vec() {
        let lock = RwLock::new(vec![1, 2, 3]);
        let guard = lock.lock_exclusive();

        assert!(guard == vec![1, 2, 3]);
        assert!(guard == [1, 2, 3].as_slice());
        assert!(guard != vec![1, 2]);
    }

    // ==================== DEBUG ====================

    #[test]
    fn test_debug() {
        let lock = RwLock::new(42);
        let guard = lock.lock_exclusive();

        let debug_str = format!("{:?}", guard);
        assert!(debug_str.contains("WatchGuardMut"));
        assert!(debug_str.contains("data"));
        assert!(debug_str.contains("lock"));
    }

    // ==================== DOWNCAST ====================

    #[test]
    fn test_downcast_success() {
        let lock = RwLock::new(Box::new(String::from("hello")) as Box<dyn Any>);

        let guard = lock.lock_exclusive();
        unsafe {
            let str_guard = WatchGuardMut::downcast::<String>(guard).unwrap();
            assert_eq!(*str_guard, "hello");
        }
    }

    #[test]
    fn test_downcast_mutate() {
        let lock = RwLock::new(Box::new(vec![1, 2, 3]) as Box<dyn Any>);

        {
            let guard = lock.lock_exclusive();
            unsafe {
                let mut vec_guard = WatchGuardMut::downcast::<Vec<i32>>(guard).unwrap();
                vec_guard.push(4);
                vec_guard[0] = 10;
            }
        }

        let guard = lock.lock_exclusive();
        unsafe {
            let vec_guard = WatchGuardMut::downcast::<Vec<i32>>(guard).unwrap();
            assert_eq!(*vec_guard, vec![10, 2, 3, 4]);
        }
    }

    #[test]
    fn test_downcast_wrong_type() {
        let lock = RwLock::new(Box::new(42i32) as Box<dyn Any>);

        let guard = lock.lock_exclusive();
        unsafe {
            let result = WatchGuardMut::downcast::<String>(guard);
            assert!(result.is_err());

            // Il guard originale viene restituito
            let original = result.unwrap_err();
            assert!(original.is_locked());
        }
    }

    #[test]
    fn test_downcast_multiple_times() {
        let lock = RwLock::new(Box::new(String::from("count:0")) as Box<dyn Any>);

        for i in 1..=5 {
            let guard = lock.lock_exclusive();
            unsafe {
                let mut str_guard = WatchGuardMut::downcast::<String>(guard).unwrap();
                *str_guard = format!("count:{}", i);
            }
        }

        let guard = lock.lock_exclusive();
        unsafe {
            let str_guard = WatchGuardMut::downcast::<String>(guard).unwrap();
            assert_eq!(*str_guard, "count:5");
        }
    }

    #[test]
    fn test_downcast_various_types() {
        // i32
        let lock_i32 = RwLock::new(Box::new(42i32) as Box<dyn Any>);
        let guard = lock_i32.lock_exclusive();
        unsafe {
            let g = WatchGuardMut::downcast::<i32>(guard).unwrap();
            assert_eq!(*g, 42);
        }

        // f64
        let lock_f64 = RwLock::new(Box::new(3.14f64) as Box<dyn Any>);
        let guard = lock_f64.lock_exclusive();
        unsafe {
            let g = WatchGuardMut::downcast::<f64>(guard).unwrap();
            assert!((*g - 3.14).abs() < 0.001);
        }

        // bool
        let lock_bool = RwLock::new(Box::new(true) as Box<dyn Any>);
        let guard = lock_bool.lock_exclusive();
        unsafe {
            let g = WatchGuardMut::downcast::<bool>(guard).unwrap();
            assert!(*g);
        }
    }

    // ==================== CONCURRENT ====================

    #[test]
    fn test_concurrent_exclusive_access() {
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
        assert_eq!(*lock.lock_exclusive(), 2000);
    }

    #[test]
    fn test_concurrent_downcast() {
        // Usa dyn Any + Send + Sync per thread safety
        let lock = Arc::new(RwLock::new(Box::new(0i64) as Box<dyn Any + Send + Sync>));
        let barrier = Arc::new(Barrier::new(4));

        let handles: Vec<_> = (0..4)
            .map(|_| {
                let l = lock.clone();
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                    for _ in 0..200 {
                        let mut guard = l.lock_exclusive();
                        // Downcast diretto senza usare WatchGuardMut::downcast
                        // perché il tipo è Box<dyn Any + Send + Sync>
                        if let Some(val) = (**guard).downcast_mut::<i64>() {
                            *val += 1;
                        }
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        let guard = lock.lock_exclusive();
        let val = (**guard).downcast_ref::<i64>().unwrap();
        assert_eq!(*val, 800);
    }

    // ==================== EDGE CASES ====================

    #[test]
    fn test_zero_sized_type() {
        let lock = RwLock::new(());

        {
            let _guard = lock.lock_exclusive();
        }

        let _guard2 = lock.lock_exclusive();
    }

    #[test]
    fn test_nested_lock_value() {
        let inner = Arc::new(RwLock::new(0));
        let lock = RwLock::new(inner.clone());

        {
            let guard = lock.lock_exclusive();
            let mut inner_guard = guard.lock_exclusive();
            *inner_guard = 42;
        }

        assert_eq!(*inner.lock_exclusive(), 42);
    }

    #[test]
    fn test_option_value() {
        let lock = RwLock::new(None::<i32>);

        {
            let mut guard = lock.lock_exclusive();
            *guard = Some(42);
        }

        {
            let guard = lock.lock_exclusive();
            assert_eq!(*guard, Some(42));
        }
    }

    #[test]
    fn test_result_value() {
        let lock = RwLock::new(Ok::<i32, &str>(0));

        {
            let mut guard = lock.lock_exclusive();
            *guard = Err("error");
        }

        {
            let guard = lock.lock_exclusive();
            assert!(guard.is_err());
        }
    }

    // ==================== GUARD SEMANTICS ====================

    #[test]
    fn test_must_use_guard() {
        let lock = RwLock::new(42);

        // Il guard deve essere usato o assegnato
        let guard = lock.lock_exclusive();
        assert_eq!(*guard, 42);
    }

    #[test]
    fn test_sequential_guards() {
        let lock = RwLock::new(0);

        for i in 1..=10 {
            let mut guard = lock.lock_exclusive();
            *guard = i;
        }

        assert_eq!(*lock.lock_exclusive(), 10);
    }

    #[test]
    fn test_guard_with_complex_mutation() {
        let lock = RwLock::new(std::collections::HashMap::<String, i32>::new());

        {
            let mut guard = lock.lock_exclusive();
            guard.insert("a".to_string(), 1);
            guard.insert("b".to_string(), 2);
            guard.insert("c".to_string(), 3);
        }

        {
            let mut guard = lock.lock_exclusive();
            *guard.get_mut("a").unwrap() = 10;
            guard.remove("c");
        }

        {
            let guard = lock.lock_exclusive();
            assert_eq!(guard.len(), 2);
            assert_eq!(guard.get("a"), Some(&10));
            assert_eq!(guard.get("b"), Some(&2));
            assert!(guard.get("c").is_none());
        }
    }
}