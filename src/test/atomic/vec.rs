#[cfg(test)]
mod tests_atomic_vec {
    use crate::atomic::AtomicVec;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier, Mutex};
    use std::thread;

    // ==================== TRACKED WRAPPER FOR MEMORY SAFETY ====================

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
            Self {
                id: self.id,
                counter: self.counter.clone(),
            }
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

    // ==================== BASIC FUNCTIONALITY ====================

    #[test]
    fn test_new_empty() {
        let vec: AtomicVec<i32> = AtomicVec::new();
        assert_eq!(vec.len(), 0);
        assert!(vec.is_empty());
    }

    #[test]
    fn test_with_capacity() {
        let vec: AtomicVec<i32> = AtomicVec::with_capacity(100);
        assert!(vec.is_empty());
        for i in 0..100 {
            vec.push(i);
        }
        assert_eq!(vec.len(), 100);
    }

    #[test]
    fn test_default_trait() {
        let vec: AtomicVec<String> = AtomicVec::default();
        assert!(vec.is_empty());
    }

    // ==================== PUSH ====================

    #[test]
    fn test_push_single() {
        let vec = AtomicVec::new();
        vec.push(42);
        assert_eq!(vec.len(), 1);
        assert!(!vec.is_empty());
    }

    #[test]
    fn test_push_multiple() {
        let vec = AtomicVec::new();
        for i in 0..100 {
            vec.push(i);
        }
        assert_eq!(vec.len(), 100);
    }

    #[test]
    fn test_push_preserves_order() {
        let vec = AtomicVec::new();
        for i in 0..10 {
            vec.push(i * 10);
        }
        for i in 0..10 {
            assert_eq!(vec.pop(), Some(i * 10));
        }
    }

    // ==================== POP ====================

    #[test]
    fn test_pop_single() {
        let vec = AtomicVec::new();
        vec.push(42);
        assert_eq!(vec.pop(), Some(42));
        assert!(vec.is_empty());
    }

    #[test]
    fn test_pop_empty() {
        let vec: AtomicVec<i32> = AtomicVec::new();
        assert_eq!(vec.pop(), None);
    }

    #[test]
    fn test_pop_fifo_order() {
        let vec = AtomicVec::new();
        vec.push(1);
        vec.push(2);
        vec.push(3);
        assert_eq!(vec.pop(), Some(1));
        assert_eq!(vec.pop(), Some(2));
        assert_eq!(vec.pop(), Some(3));
        assert_eq!(vec.pop(), None);
    }

    #[test]
    fn test_pop_until_empty() {
        let vec = AtomicVec::new();
        for i in 0..50 {
            vec.push(i);
        }
        for i in 0..50 {
            assert_eq!(vec.pop(), Some(i));
        }
        assert_eq!(vec.pop(), None);
        assert!(vec.is_empty());
    }

    // ==================== PUSH_BATCH ====================

    #[test]
    fn test_push_batch_range() {
        let vec = AtomicVec::new();
        vec.push_batch(0..100);
        assert_eq!(vec.len(), 100);
    }

    #[test]
    fn test_push_batch_vec() {
        let vec = AtomicVec::new();
        vec.push_batch(vec![1, 2, 3, 4, 5]);
        assert_eq!(vec.len(), 5);
        assert_eq!(vec.pop(), Some(1));
    }

    #[test]
    fn test_push_batch_empty() {
        let vec = AtomicVec::new();
        vec.push_batch(std::iter::empty::<i32>());
        assert!(vec.is_empty());
    }

    // ==================== POP_BATCH ====================

    #[test]
    fn test_pop_batch() {
        let vec = AtomicVec::new();
        vec.push_batch(0..100);

        let batch = vec.pop_batch(50);
        assert_eq!(batch.len(), 50);
        assert_eq!(vec.len(), 50);
    }

    #[test]
    fn test_pop_batch_more_than_available() {
        let vec = AtomicVec::new();
        vec.push_batch(0..10);

        let batch = vec.pop_batch(100);
        assert_eq!(batch.len(), 10);
        assert!(vec.is_empty());
    }

    #[test]
    fn test_pop_batch_zero() {
        let vec = AtomicVec::new();
        vec.push_batch(0..10);

        let batch = vec.pop_batch(0);
        assert!(batch.is_empty());
        assert_eq!(vec.len(), 10);
    }

    // ==================== DRAIN ====================

    #[test]
    fn test_drain_all() {
        let vec = AtomicVec::new();
        vec.push_batch(0..50);

        let drained = vec.drain();
        assert_eq!(drained.len(), 50);
        assert!(vec.is_empty());
    }

    #[test]
    fn test_drain_empty() {
        let vec: AtomicVec<i32> = AtomicVec::new();
        let drained = vec.drain();
        assert!(drained.is_empty());
    }

    // ==================== INIT_WITH ====================

    #[test]
    fn test_init_with_constant() {
        let vec: AtomicVec<i32> = AtomicVec::init_with(5, || 42);
        assert_eq!(vec.len(), 5);
        for _ in 0..5 {
            assert_eq!(vec.pop(), Some(42));
        }
    }

    #[test]
    fn test_init_with_closure_state() {
        let mut counter = 0;
        let vec: AtomicVec<i32> = AtomicVec::init_with(5, || {
            counter += 1;
            counter
        });
        assert_eq!(vec.pop(), Some(1));
        assert_eq!(vec.pop(), Some(2));
    }

    // ==================== RESET_WITH ====================

    #[test]
    fn test_reset_with() {
        let vec = AtomicVec::new();
        vec.push_batch(0..10);

        let mut counter = 0;
        vec.reset_with(5, || {
            counter += 1;
            counter * 10
        });

        assert_eq!(vec.len(), 5);
        assert_eq!(vec.pop(), Some(10));
        assert_eq!(vec.pop(), Some(20));
    }

    #[test]
    fn test_reset_with_empty() {
        let vec: AtomicVec<i32> = AtomicVec::new();
        vec.reset_with(3, || 99);
        assert_eq!(vec.len(), 3);
    }

    // ==================== AS_VEC ====================

    #[test]
    fn test_as_vec_empty() {
        let vec: AtomicVec<i32> = AtomicVec::new();
        assert!(vec.as_vec().is_empty());
    }

    #[test]
    fn test_as_vec_preserves_order() {
        let vec = AtomicVec::new();
        vec.push_batch(vec![5, 4, 3, 2, 1]);
        assert_eq!(vec.as_vec(), vec![5, 4, 3, 2, 1]);
    }

    #[test]
    fn test_as_vec_after_pop() {
        let vec = AtomicVec::new();
        vec.push_batch(0..10);
        vec.pop();
        vec.pop();
        assert_eq!(vec.as_vec(), (2..10).collect::<Vec<_>>());
    }

    #[test]
    fn test_as_vec_clones_independently() {
        let vec = AtomicVec::new();
        vec.push("hello".to_string());

        let snapshot = vec.as_vec();
        vec.pop();
        vec.push("world".to_string());

        assert_eq!(snapshot, vec!["hello".to_string()]);
    }

    // ==================== FROM_ITER ====================

    #[test]
    fn test_from_iter_range() {
        let vec: AtomicVec<i32> = (0..10).collect();
        assert_eq!(vec.len(), 10);
        for i in 0..10 {
            assert_eq!(vec.pop(), Some(i));
        }
    }

    #[test]
    fn test_from_iter_vec() {
        let vec: AtomicVec<String> = vec!["a", "b", "c"].into_iter().map(String::from).collect();
        assert_eq!(vec.len(), 3);
    }

    #[test]
    fn test_from_iter_empty() {
        let vec: AtomicVec<i32> = std::iter::empty().collect();
        assert!(vec.is_empty());
    }

    // ==================== CLONE ====================

    #[test]
    fn test_clone_shares_data() {
        let vec = AtomicVec::new();
        vec.push(42);

        let clone = vec.clone();
        assert_eq!(clone.len(), 1);
        assert_eq!(clone.pop(), Some(42));

        // Original should also be affected
        assert!(vec.is_empty());
    }

    #[test]
    fn test_clone_push_visible() {
        let vec = AtomicVec::new();
        let clone = vec.clone();

        vec.push(100);
        assert_eq!(clone.len(), 1);
    }

    #[test]
    fn test_multiple_clones() {
        let vec = AtomicVec::new();
        vec.push(1);

        let c1 = vec.clone();
        let c2 = vec.clone();
        let c3 = c1.clone();

        drop(c1);
        drop(c2);
        assert_eq!(c3.pop(), Some(1));
    }

    // ==================== LEN / IS_EMPTY / CAPACITY ====================

    #[test]
    fn test_len_updates() {
        let vec = AtomicVec::new();
        assert_eq!(vec.len(), 0);

        vec.push(1);
        assert_eq!(vec.len(), 1);

        vec.push(2);
        vec.push(3);
        assert_eq!(vec.len(), 3);

        vec.pop();
        assert_eq!(vec.len(), 2);
    }

    #[test]
    fn test_is_empty() {
        let vec: AtomicVec<i32> = AtomicVec::new();
        assert!(vec.is_empty());

        vec.push(1);
        assert!(!vec.is_empty());

        vec.pop();
        assert!(vec.is_empty());
    }

    #[test]
    fn test_capacity() {
        let vec: AtomicVec<i32> = AtomicVec::with_capacity(100);
        // capacity() returns current block capacity, not requested
        assert!(vec.capacity() > 0);
    }

    // ==================== LEAK DETECTION ====================

    #[test]
    fn test_no_leak_on_drop() {
        let counter = Arc::new(AtomicUsize::new(0));
        {
            let vec = AtomicVec::new();
            for i in 0..50 {
                vec.push(Tracked::new(i, counter.clone()));
            }
            assert_eq!(counter.load(Ordering::SeqCst), 50);
        }
        assert_eq!(counter.load(Ordering::SeqCst), 0, "Memory leak detected");
    }

    #[test]
    fn test_no_leak_with_clones() {
        let counter = Arc::new(AtomicUsize::new(0));
        {
            let vec = AtomicVec::new();
            for i in 0..5 {
                vec.push(Tracked::new(i, counter.clone()));
            }
            let c1 = vec.clone();
            let c2 = vec.clone();
            drop(c1);
            drop(c2);
            assert_eq!(counter.load(Ordering::SeqCst), 5);
        }
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_no_leak_pop() {
        let counter = Arc::new(AtomicUsize::new(0));
        let vec = AtomicVec::new();

        for i in 0..10 {
            vec.push(Tracked::new(i, counter.clone()));
        }
        assert_eq!(counter.load(Ordering::SeqCst), 10);

        for _ in 0..10 {
            let item = vec.pop();
            drop(item);
        }
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_no_leak_drain() {
        let counter = Arc::new(AtomicUsize::new(0));
        {
            let vec = AtomicVec::new();
            for i in 0..20 {
                vec.push(Tracked::new(i, counter.clone()));
            }
            let drained = vec.drain();
            assert_eq!(counter.load(Ordering::SeqCst), 20);
            drop(drained);
            assert_eq!(counter.load(Ordering::SeqCst), 0);
        }
    }

    #[test]
    fn test_no_leak_pop_batch() {
        let counter = Arc::new(AtomicUsize::new(0));
        let vec = AtomicVec::new();

        for i in 0..20 {
            vec.push(Tracked::new(i, counter.clone()));
        }

        let batch = vec.pop_batch(10);
        assert_eq!(counter.load(Ordering::SeqCst), 20);
        drop(batch);
        assert_eq!(counter.load(Ordering::SeqCst), 10);

        drop(vec);
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_no_leak_reset_with() {
        let counter = Arc::new(AtomicUsize::new(0));
        let vec = AtomicVec::new();

        for i in 0..10 {
            vec.push(Tracked::new(i, counter.clone()));
        }
        assert_eq!(counter.load(Ordering::SeqCst), 10);

        let c = counter.clone();
        vec.reset_with(5, || Tracked::new(999, c.clone()));
        assert_eq!(counter.load(Ordering::SeqCst), 5);

        drop(vec);
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_no_leak_from_iter() {
        let counter = Arc::new(AtomicUsize::new(0));
        {
            let items: Vec<_> = (0..20).map(|i| Tracked::new(i, counter.clone())).collect();
            let vec: AtomicVec<_> = items.into_iter().collect();
            assert_eq!(vec.len(), 20);
        }
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    // ==================== CONCURRENT TESTS ====================

    #[test]
    fn test_concurrent_push_pop() {
        let vec = Arc::new(AtomicVec::new());
        vec.push_batch(0..500);

        let barrier = Arc::new(Barrier::new(20));

        // 10 pushers
        let pushers: Vec<_> = (0..10)
            .map(|t| {
                let vec = vec.clone();
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                    for i in 0..50 {
                        vec.push(1000 + t * 50 + i);
                    }
                })
            })
            .collect();

        // 10 poppers
        let pop_count = Arc::new(AtomicUsize::new(0));
        let poppers: Vec<_> = (0..10)
            .map(|_| {
                let vec = vec.clone();
                let b = barrier.clone();
                let count = pop_count.clone();
                thread::spawn(move || {
                    b.wait();
                    for _ in 0..50 {
                        if vec.pop().is_some() {
                            count.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                })
            })
            .collect();

        for h in pushers {
            h.join().unwrap();
        }
        for h in poppers {
            h.join().unwrap();
        }

        let popped = pop_count.load(Ordering::SeqCst);
        assert_eq!(vec.len(), 500 + 500 - popped);
    }

    #[test]
    fn test_concurrent_clone_drop() {
        let counter = Arc::new(AtomicUsize::new(0));
        let vec = Arc::new(AtomicVec::new());

        for i in 0..50 {
            vec.push(Tracked::new(i, counter.clone()));
        }

        let barrier = Arc::new(Barrier::new(20));
        let handles: Vec<_> = (0..20)
            .map(|_| {
                let vec_clone = (*vec).clone();
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                    thread::yield_now();
                    drop(vec_clone);
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }
        drop(vec);

        thread::sleep(std::time::Duration::from_millis(10));
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_concurrent_push_batch() {
        let vec = Arc::new(AtomicVec::new());
        let barrier = Arc::new(Barrier::new(10));

        let handles: Vec<_> = (0..10)
            .map(|t| {
                let vec = vec.clone();
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                    vec.push_batch((t * 100)..((t + 1) * 100));
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(vec.len(), 1000);
    }

    // ==================== STRESS TESTS ====================

    #[test]
    fn stress_push_pop_drop() {
        let counter = Arc::new(AtomicUsize::new(0));

        for iter in 0..10 {
            let vec = Arc::new(AtomicVec::new());
            let barrier = Arc::new(Barrier::new(30));
            let c = counter.clone();

            // 10 pushers
            let pushers: Vec<_> = (0..10)
                .map(|t| {
                    let vec = vec.clone();
                    let c = c.clone();
                    let b = barrier.clone();
                    thread::spawn(move || {
                        b.wait();
                        for i in 0..50 {
                            vec.push(Tracked::new(t * 50 + i, c.clone()));
                        }
                    })
                })
                .collect();

            // 10 poppers
            let poppers: Vec<_> = (0..10)
                .map(|_| {
                    let vec = vec.clone();
                    let b = barrier.clone();
                    thread::spawn(move || {
                        b.wait();
                        for _ in 0..30 {
                            if let Some(item) = vec.pop() {
                                drop(item);
                            }
                        }
                    })
                })
                .collect();

            // 10 droppers
            let droppers: Vec<_> = (0..10)
                .map(|_| {
                    let vec = vec.clone();
                    let b = barrier.clone();
                    thread::spawn(move || {
                        b.wait();
                        thread::sleep(std::time::Duration::from_micros(50));
                        drop(vec);
                    })
                })
                .collect();

            for h in pushers {
                h.join().unwrap();
            }
            for h in poppers {
                h.join().unwrap();
            }
            for h in droppers {
                h.join().unwrap();
            }

            drop(vec);
            assert_eq!(counter.load(Ordering::SeqCst), 0, "Leak at iter {}", iter);
        }
    }

    #[test]
    fn stress_rapid_clone_drop() {
        let counter = Arc::new(AtomicUsize::new(0));

        for _ in 0..50 {
            let vec = AtomicVec::new();
            for i in 0..10 {
                vec.push(Tracked::new(i, counter.clone()));
            }

            let clones: Vec<_> = (0..100).map(|_| vec.clone()).collect();
            let handles: Vec<_> = clones
                .into_iter()
                .map(|c| {
                    thread::spawn(move || {
                        thread::yield_now();
                        drop(c);
                    })
                })
                .collect();

            for h in handles {
                h.join().unwrap();
            }
            drop(vec);
        }

        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn stress_mixed_operations() {
        let vec = Arc::new(AtomicVec::new());
        vec.push_batch(0..200);

        let barrier = Arc::new(Barrier::new(40));

        // 10 pushers
        let pushers: Vec<_> = (0..10)
            .map(|t| {
                let vec = vec.clone();
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                    for i in 0..20 {
                        vec.push(1000 + t * 20 + i);
                    }
                })
            })
            .collect();

        // 10 poppers
        let poppers: Vec<_> = (0..10)
            .map(|_| {
                let vec = vec.clone();
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                    for _ in 0..15 {
                        vec.pop();
                    }
                })
            })
            .collect();

        // 10 batch pushers
        let batch_pushers: Vec<_> = (0..10)
            .map(|t| {
                let vec = vec.clone();
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                    vec.push_batch((t * 10 + 2000)..((t + 1) * 10 + 2000));
                })
            })
            .collect();

        // 10 as_vec readers
        let readers: Vec<_> = (0..10)
            .map(|_| {
                let vec = vec.clone();
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                    for _ in 0..5 {
                        let _ = vec.as_vec();
                        let _ = vec.len();
                    }
                })
            })
            .collect();

        for h in pushers {
            h.join().unwrap();
        }
        for h in poppers {
            h.join().unwrap();
        }
        for h in batch_pushers {
            h.join().unwrap();
        }
        for h in readers {
            h.join().unwrap();
        }
    }

    // ==================== EDGE CASES ====================

    #[test]
    fn test_empty_operations() {
        let vec: AtomicVec<i32> = AtomicVec::new();

        assert!(vec.pop().is_none());
        assert!(vec.drain().is_empty());
        assert!(vec.pop_batch(10).is_empty());
        assert!(vec.as_vec().is_empty());
    }

    #[test]
    fn test_single_element() {
        let vec = AtomicVec::new();
        vec.push(42);

        assert_eq!(vec.len(), 1);
        assert_eq!(vec.pop(), Some(42));
        assert!(vec.is_empty());
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_large_capacity() {
        let vec = AtomicVec::new();
        for i in 0..10000 {
            vec.push(i);
        }
        assert_eq!(vec.len(), 10000);

        // Pop all and verify count
        let mut count = 0;
        while vec.pop().is_some() {
            count += 1;
        }
        assert_eq!(count, 10000);
    }

    #[test]
    fn test_zero_sized_type() {
        let vec: AtomicVec<()> = AtomicVec::new();
        for _ in 0..100 {
            vec.push(());
        }
        assert_eq!(vec.len(), 100);
        assert!(vec.pop().is_some());
    }

    #[test]
    fn test_drop_tracking() {
        let drop_order = Arc::new(Mutex::new(Vec::new()));

        struct OrderTracker {
            id: usize,
            order: Arc<Mutex<Vec<usize>>>,
        }

        impl Drop for OrderTracker {
            fn drop(&mut self) {
                self.order.lock().unwrap().push(self.id);
            }
        }

        {
            let vec = AtomicVec::new();
            for i in 0..5 {
                vec.push(OrderTracker {
                    id: i,
                    order: drop_order.clone(),
                });
            }
        }

        assert_eq!(drop_order.lock().unwrap().len(), 5);
    }

    #[test]
    fn test_interleaved_push_pop() {
        let vec = AtomicVec::new();

        // Push some, pop some, verify len is correct
        for round in 0..10 {
            for _ in 0..10 {
                vec.push(round);
            }
            for _ in 0..5 {
                assert!(vec.pop().is_some());
            }
        }

        // Should have 50 elements left (5 per round * 10 rounds)
        assert_eq!(vec.len(), 50);
    }

    #[test]
    fn test_push_pop_cycles() {
        let vec = AtomicVec::new();

        // Multiple cycles of push and pop
        for _ in 0..100 {
            vec.push(1);
            vec.push(2);
            vec.push(3);
            assert!(vec.pop().is_some());
            assert!(vec.pop().is_some());
        }

        // Should have 100 elements (one per iteration)
        assert_eq!(vec.len(), 100);
    }
}
