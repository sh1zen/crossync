mod tests_atomic_array {
    use crate::atomic::AtomicArray;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::Duration;

    // ==================== TRACKED WRAPPER ====================

    #[derive(Debug)]
    struct Tracked {
        id: usize,
        counter: Arc<AtomicUsize>,
    }

    impl Tracked {
        fn new(id: usize, counter: Arc<AtomicUsize>) -> Self {
            counter.fetch_add(1, Ordering::SeqCst);
            Self { id, counter }
        }
    }

    impl Clone for Tracked {
        fn clone(&self) -> Self {
            self.counter.fetch_add(1, Ordering::SeqCst);
            Self { id: self.id, counter: self.counter.clone() }
        }
    }

    impl Drop for Tracked {
        fn drop(&mut self) {
            let prev = self.counter.fetch_sub(1, Ordering::SeqCst);
            if prev == 0 {
                panic!("Double free detected for id {}", self.id);
            }
        }
    }

    // ==================== CONSTRUCTION ====================

    #[test]
    fn test_new_and_default() {
        let arr1: AtomicArray<i32> = AtomicArray::new();
        let arr2: AtomicArray<i32> = AtomicArray::default();

        for arr in [&arr1, &arr2] {
            assert_eq!(arr.len(), 0);
            assert_eq!(arr.capacity(), 32);
            assert!(arr.is_empty());
        }
    }

    #[test]
    fn test_with_capacity() {
        let arr: AtomicArray<i32> = AtomicArray::with_capacity(100);
        assert_eq!(arr.capacity(), 100);
        assert_eq!(arr.len(), 0);
    }

    #[test]
    #[should_panic(expected = "Capacity must be greater than 0")]
    fn test_zero_capacity_panics() {
        let _: AtomicArray<i32> = AtomicArray::with_capacity(0);
    }

    #[test]
    fn test_init_with() {
        let mut counter = 0;
        let arr: AtomicArray<i32> = AtomicArray::init_with(5, || {
            counter += 1;
            counter * 10
        });
        assert_eq!(arr.len(), 5);
        assert_eq!(arr.as_vec(), vec![10, 20, 30, 40, 50]);
    }

    #[test]
    fn test_from_iter() {
        let arr: AtomicArray<i32> = (0..5).collect();
        assert_eq!(arr.len(), 5);
        assert_eq!(arr.as_vec(), vec![0, 1, 2, 3, 4]);

        let empty: AtomicArray<i32> = std::iter::empty().collect();
        assert!(empty.is_empty());
    }

    #[test]
    fn test_debug_trait() {
        let arr = AtomicArray::from_iter(vec![1, 2, 3]);
        let debug_str = format!("{:?}", arr);
        assert!(debug_str.contains("AtomicArray"));
        assert!(debug_str.contains("len"));
    }

    // ==================== PUSH ====================

    #[test]
    fn test_push() {
        let arr = AtomicArray::with_capacity(3);

        // Push validi
        for i in 0..3 {
            assert!(arr.push(i * 10).is_ok());
        }
        assert_eq!(arr.len(), 3);
        assert_eq!(arr.as_vec(), vec![0, 10, 20]);

        // Push oltre capacità
        let result = arr.push(99);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), 99);
        assert_eq!(arr.len(), 3);
    }

    // ==================== GET / GET_MUT ====================

    #[test]
    fn test_get() {
        let arr = AtomicArray::from_iter(vec!["hello".to_string(), "world".to_string()]);

        // Accesso valido
        let guard = arr.get(0).unwrap();
        assert_eq!(guard.len(), 5);
        assert!(guard.starts_with("hel"));
        drop(guard);

        // Out of bounds
        assert!(arr.get(2).is_none());
        assert!(arr.get(usize::MAX).is_none());
    }

    #[test]
    fn test_get_mut() {
        let arr = AtomicArray::from_iter(vec![vec![1, 2], vec![3, 4]]);

        // Modifica
        arr.get_mut(0).unwrap().push(99);
        assert_eq!(*arr.get(0).unwrap(), vec![1, 2, 99]);

        // Out of bounds
        assert!(arr.get_mut(2).is_none());
    }

    #[test]
    fn test_with() {
        let arr = AtomicArray::from_iter(vec![10, 20, 30]);

        assert_eq!(arr.with(1, |value| *value), Some(20));
        assert_eq!(arr.with(10, |value| *value), None);
    }

    #[test]
    fn test_with_mut() {
        let arr = AtomicArray::from_iter(vec![10, 20, 30]);

        let updated = arr.with_mut(1, |value| {
            *value += 5;
            *value
        });

        assert_eq!(updated, Some(25));
        assert_eq!(arr.with(1, |value| *value), Some(25));
        assert_eq!(arr.with_mut(10, |value| *value), None);
    }

    // ==================== RESET_WITH ====================

    #[test]
    fn test_reset_with() {
        let arr: AtomicArray<i32> = AtomicArray::from_iter(0..10);

        let result = arr.reset_with(5, || 99);
        assert_eq!(result, Ok(5));
        assert_eq!(arr.len(), 5);
        assert_eq!(arr.capacity(), 5);
        assert_eq!(arr.as_vec(), vec![99; 5]);
    }

    #[test]
    #[should_panic(expected = "Capacity must be greater than 0")]
    fn test_reset_with_zero_panics() {
        let arr: AtomicArray<i32> = AtomicArray::new();
        let _ = arr.reset_with(0, || 0);
    }

    #[test]
    fn test_reset_with_waits_for_live_guard() {
        let arr = Arc::new(AtomicArray::from_iter(0..4));
        let guard_ready = Arc::new(Barrier::new(2));
        let release_guard = Arc::new(AtomicBool::new(false));
        let reset_done = Arc::new(AtomicBool::new(false));

        let reader_arr = arr.clone();
        let reader_ready = guard_ready.clone();
        let reader_release = release_guard.clone();
        let reader = thread::spawn(move || {
            let guard = reader_arr.get(0).unwrap();
            reader_ready.wait();
            while !reader_release.load(Ordering::Acquire) {
                thread::yield_now();
            }
            drop(guard);
        });

        guard_ready.wait();

        let reset_arr = arr.clone();
        let done = reset_done.clone();
        let resetter = thread::spawn(move || {
            reset_arr.reset_with(4, || 99).unwrap();
            done.store(true, Ordering::Release);
        });

        thread::sleep(Duration::from_millis(20));
        assert!(
            !reset_done.load(Ordering::Acquire),
            "reset_with completed while a slot guard was still alive"
        );

        release_guard.store(true, Ordering::Release);
        reader.join().unwrap();
        resetter.join().unwrap();

        assert_eq!(arr.as_vec(), vec![99; 4]);
    }

    #[test]
    fn test_reset_with_concurrent_accessors_and_push() {
        // Bound the workload so readers eventually quiesce and reset can acquire
        // exclusive coordination even on a heavily contended CI runner. This
        // also bounds the number of increments of the i32 test value.
        const ACCESS_ATTEMPTS: usize = 1024;

        let arr = Arc::new(AtomicArray::from_iter(0..32));
        let start = Arc::new(Barrier::new(7));

        let reset_arr = arr.clone();
        let reset_start = start.clone();
        let resetter = thread::spawn(move || {
            reset_start.wait();
            for round in 0..64 {
                reset_arr.reset_with(32, || round).unwrap();
                thread::yield_now();
            }
        });

        let get_arr = arr.clone();
        let get_start = start.clone();
        let getter = thread::spawn(move || {
            get_start.wait();
            for _ in 0..ACCESS_ATTEMPTS {
                if let Some(guard) = get_arr.get(0) {
                    let _ = *guard;
                }
            }
        });

        let get_mut_arr = arr.clone();
        let get_mut_start = start.clone();
        let mutator = thread::spawn(move || {
            get_mut_start.wait();
            for _ in 0..ACCESS_ATTEMPTS {
                if let Some(mut guard) = get_mut_arr.get_mut(0) {
                    *guard += 1;
                }
            }
        });

        let as_vec_arr = arr.clone();
        let as_vec_start = start.clone();
        let snapshotter = thread::spawn(move || {
            as_vec_start.wait();
            for _ in 0..ACCESS_ATTEMPTS {
                let _ = as_vec_arr.as_vec();
            }
        });

        let for_each_arr = arr.clone();
        let for_each_start = start.clone();
        let iterator = thread::spawn(move || {
            for_each_start.wait();
            for _ in 0..ACCESS_ATTEMPTS {
                for_each_arr.for_each(|_| {});
            }
        });

        let push_arr = arr.clone();
        let push_start = start.clone();
        let pusher = thread::spawn(move || {
            push_start.wait();
            for _ in 0..ACCESS_ATTEMPTS {
                let _ = push_arr.push(1234);
            }
        });

        start.wait();

        resetter.join().unwrap();
        getter.join().unwrap();
        mutator.join().unwrap();
        snapshotter.join().unwrap();
        iterator.join().unwrap();
        pusher.join().unwrap();

        assert_eq!(arr.len(), 32);
        assert_eq!(arr.capacity(), 32);
    }

    // ==================== AS_VEC ====================

    #[test]
    fn test_as_vec() {
        // Vuoto
        let empty: AtomicArray<i32> = AtomicArray::new();
        assert_eq!(empty.as_vec(), Vec::<i32>::new());

        // Con elementi - verifica indipendenza della copia
        let arr = AtomicArray::from_iter(vec!["a".to_string()]);
        let vec = arr.as_vec();
        *arr.get_mut(0).unwrap() = "modified".to_string();
        assert_eq!(vec[0], "a");
    }

    // ==================== FOR_EACH / FOR_EACH_MUT ====================

    #[test]
    fn test_for_each() {
        let arr = AtomicArray::from_iter(1..=5);
        let mut sum = 0;
        let mut order = Vec::new();

        arr.for_each(|v| {
            sum += *v;
            order.push(*v);
        });

        assert_eq!(sum, 15);
        assert_eq!(order, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_for_each_mut() {
        let arr = AtomicArray::from_iter(vec![1, 2, 3, 4, 5]);
        arr.for_each_mut(|v| *v *= 2);
        assert_eq!(arr.as_vec(), vec![2, 4, 6, 8, 10]);
    }

    // ==================== CHUNK_INDICES ====================

    #[test]
    fn test_chunk_indices() {
        // Divisione pari
        let arr = AtomicArray::from_iter(0..100);
        assert_eq!(arr.chunk_indices(4), vec![(0, 25), (25, 50), (50, 75), (75, 100)]);

        // Divisione dispari
        let arr = AtomicArray::from_iter(0..10);
        assert_eq!(arr.chunk_indices(3), vec![(0, 4), (4, 8), (8, 10)]);

        // Più chunk che elementi
        let arr = AtomicArray::from_iter(0..3);
        assert_eq!(arr.chunk_indices(10), vec![(0, 1), (1, 2), (2, 3)]);

        // Edge cases
        let empty: AtomicArray<i32> = AtomicArray::new();
        assert!(empty.chunk_indices(4).is_empty());
        assert!(arr.chunk_indices(0).is_empty());
    }

    // ==================== CLONE ====================

    #[test]
    fn test_clone_shares_data() {
        let arr = AtomicArray::from_iter(vec![10]);
        let c1 = arr.clone();
        let c2 = c1.clone();

        // Mutazione visibile attraverso tutti i clone
        *arr.get_mut(0).unwrap() = 999;
        assert_eq!(*c1.get(0).unwrap(), 999);
        assert_eq!(*c2.get(0).unwrap(), 999);

        // Drop parziale non invalida gli altri
        drop(c1);
        assert_eq!(*c2.get(0).unwrap(), 999);
    }

    // ==================== MEMORY SAFETY ====================

    #[test]
    fn test_no_leaks() {
        let counter = Arc::new(AtomicUsize::new(0));

        {
            // Test drop normale
            let arr = AtomicArray::with_capacity(20);
            for i in 0..10 {
                arr.push(Tracked::new(i, counter.clone())).unwrap();
            }
            assert_eq!(counter.load(Ordering::SeqCst), 10);

            // Test con cloni
            let c1 = arr.clone();
            let c2 = arr.clone();
            drop(c1);
            drop(c2);
            assert_eq!(counter.load(Ordering::SeqCst), 10);
        }
        assert_eq!(counter.load(Ordering::SeqCst), 0, "Memory leak detected");

        // Test from_iter
        {
            let items: Vec<_> = (0..5).map(|i| Tracked::new(i, counter.clone())).collect();
            let _arr: AtomicArray<_> = items.into_iter().collect();
        }
        assert_eq!(counter.load(Ordering::SeqCst), 0, "Leak in from_iter");
    }

    #[test]
    fn test_no_double_free() {
        let counter = Arc::new(AtomicUsize::new(0));

        // Reset non causa double free
        let arr: AtomicArray<Tracked> = AtomicArray::with_capacity(20);
        for i in 0..10 {
            arr.push(Tracked::new(i, counter.clone())).unwrap();
        }

        let c = counter.clone();
        arr.reset_with(5, || Tracked::new(999, c.clone())).unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 5);

        // Push rifiutato non causa double free
        let arr2 = AtomicArray::with_capacity(2);
        arr2.push(Tracked::new(0, counter.clone())).unwrap();
        arr2.push(Tracked::new(1, counter.clone())).unwrap();

        let rejected = arr2.push(Tracked::new(2, counter.clone()));
        assert!(rejected.is_err());
        drop(rejected);
        assert_eq!(counter.load(Ordering::SeqCst), 5 + 2); // 5 da reset + 2 da arr2
    }

    // ==================== EDGE CASES ====================

    #[test]
    fn test_empty_array_operations() {
        let arr: AtomicArray<i32> = AtomicArray::new();

        assert!(arr.get(0).is_none());
        assert!(arr.get_mut(0).is_none());
        assert!(arr.as_vec().is_empty());
        assert!(arr.chunk_indices(4).is_empty());

        let mut called = false;
        arr.for_each(|_| called = true);
        arr.for_each_mut(|_| called = true);
        assert!(!called);
    }

    #[test]
    fn test_single_element() {
        let arr = AtomicArray::with_capacity(1);
        arr.push(42).unwrap();

        assert_eq!(arr.len(), 1);
        assert_eq!(*arr.get(0).unwrap(), 42);
        assert!(arr.push(99).is_err());

        *arr.get_mut(0).unwrap() = 100;
        assert_eq!(*arr.get(0).unwrap(), 100);
    }

    #[test]
    fn test_large_capacity() {
        let arr = AtomicArray::with_capacity(10000);
        for i in 0..10000 {
            arr.push(i).unwrap();
        }
        assert_eq!(arr.len(), 10000);
        assert_eq!(*arr.get(9999).unwrap(), 9999);
    }

    #[test]
    fn test_zero_sized_type() {
        let arr: AtomicArray<()> = AtomicArray::with_capacity(100);
        for _ in 0..100 {
            arr.push(()).unwrap();
        }
        assert_eq!(arr.len(), 100);
        assert!(arr.get(50).is_some());
    }

    // ==================== CONCURRENT ====================

    #[test]
    fn test_concurrent_push() {
        let arr = Arc::new(AtomicArray::with_capacity(1000));
        let barrier = Arc::new(Barrier::new(10));

        let handles: Vec<_> = (0..10)
            .map(|t| {
                let arr = arr.clone();
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                    for i in 0..100 {
                        let _ = arr.push(t * 100 + i);
                    }
                })
            })
            .collect();

        for h in handles { h.join().unwrap(); }
        assert_eq!(arr.len(), 1000);
    }

    #[test]
    fn test_concurrent_read() {
        let arr = Arc::new(AtomicArray::from_iter(0..100));
        let barrier = Arc::new(Barrier::new(5));

        let handles: Vec<_> = (0..5)
            .map(|_| {
                let arr = arr.clone();
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                    let mut sum = 0usize;
                    for _ in 0..20 {
                        for i in 0..100 {
                            sum += *arr.get(i).unwrap();
                        }
                    }
                    sum
                })
            })
            .collect();

        let total: usize = handles.into_iter().map(|h| h.join().unwrap()).sum();
        assert_eq!(total, 4950 * 20 * 5);
    }

    #[test]
    fn test_concurrent_mutation() {
        let arr = Arc::new(AtomicArray::from_iter(vec![0i64; 10]));
        let barrier = Arc::new(Barrier::new(10));

        let handles: Vec<_> = (0..10)
            .map(|_| {
                let arr = arr.clone();
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                    for _ in 0..100 {
                        arr.for_each_mut(|v| *v += 1);
                    }
                })
            })
            .collect();

        for h in handles { h.join().unwrap(); }

        // Ogni elemento incrementato 10 thread * 100 iterazioni = 1000
        arr.for_each(|v| assert_eq!(*v, 1000));
    }

    #[test]
    fn test_concurrent_clone_drop_with_tracking() {
        let counter = Arc::new(AtomicUsize::new(0));
        let arr = Arc::new(AtomicArray::with_capacity(20));

        for i in 0..10 {
            arr.push(Tracked::new(i, counter.clone())).unwrap();
        }

        let barrier = Arc::new(Barrier::new(10));
        let handles: Vec<_> = (0..10)
            .map(|_| {
                let arr_clone = (*arr).clone();
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                    thread::yield_now();
                    drop(arr_clone);
                })
            })
            .collect();

        for h in handles { h.join().unwrap(); }
        drop(arr);

        assert_eq!(counter.load(Ordering::SeqCst), 0, "Memory leak in concurrent drop");
    }

    #[test]
    fn test_concurrent_mixed_operations() {
        let arr = Arc::new(AtomicArray::with_capacity(50));
        for i in 0..20 {
            arr.push(i).unwrap();
        }

        let barrier = Arc::new(Barrier::new(6));

        // 2 writer + 2 reader + 2 mutator
        let writers: Vec<_> = (0..2).map(|t| {
            let arr = arr.clone();
            let b = barrier.clone();
            thread::spawn(move || {
                b.wait();
                for i in 0..10 { let _ = arr.push(100 + t * 10 + i); }
            })
        }).collect();

        let readers: Vec<_> = (0..2).map(|_| {
            let arr = arr.clone();
            let b = barrier.clone();
            thread::spawn(move || {
                b.wait();
                for _ in 0..20 {
                    for i in 0..arr.len() { let _ = arr.get(i); }
                }
            })
        }).collect();

        let mutators: Vec<_> = (0..2).map(|_| {
            let arr = arr.clone();
            let b = barrier.clone();
            thread::spawn(move || {
                b.wait();
                for _ in 0..5 {
                    for i in 0..arr.len().min(20) {
                        if let Some(mut g) = arr.get_mut(i) { *g += 1; }
                    }
                }
            })
        }).collect();

        for h in writers { h.join().unwrap(); }
        for h in readers { h.join().unwrap(); }
        for h in mutators { h.join().unwrap(); }

        assert_eq!(arr.len(), 40);
    }
}
