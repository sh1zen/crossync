mod tests_atomic_buffer {
    use crate::atomic::AtomicBuffer;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};
    use std::thread;

    // ==================== HELPERS ====================

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

    impl Drop for Tracked {
        fn drop(&mut self) {
            let prev = self.counter.fetch_sub(1, Ordering::SeqCst);
            if prev == 0 {
                panic!("Double free detected for id {}", self.id);
            }
        }
    }

    fn to_raw(t: Tracked) -> *mut Tracked {
        Box::into_raw(Box::new(t))
    }

    unsafe fn drop_raw<T>(ptr: *mut T) {
        unsafe {
            drop(Box::from_raw(ptr));
        }
    }

    // ==================== CONSTRUCTION ====================

    #[test]
    fn test_new_and_capacity() {
        let buffer: AtomicBuffer<i32> = AtomicBuffer::new();
        assert_eq!(buffer.capacity(), 32);
        assert!(buffer.pop().is_none());

        let buffer64: AtomicBuffer<i32> = AtomicBuffer::with_capacity(64);
        assert_eq!(buffer64.capacity(), 64);
    }

    #[test]
    #[should_panic(expected = "capacity must be power of two")]
    fn test_non_power_of_two_panics() {
        let _: AtomicBuffer<i32> = AtomicBuffer::with_capacity(33);
    }

    // ==================== PUSH / POP ====================

    #[test]
    fn test_push_pop_single() {
        let buffer = AtomicBuffer::<i32>::new();
        let val = Box::into_raw(Box::new(42));

        assert!(buffer.push(val).is_ok());

        let popped = buffer.pop().unwrap();
        unsafe {
            assert_eq!(*popped, 42);
            drop_raw(popped);
        }

        assert!(buffer.pop().is_none());
    }

    #[test]
    fn test_push_pop_fifo_order() {
        let buffer = AtomicBuffer::<i32>::new();

        for i in 0..10 {
            buffer.push(Box::into_raw(Box::new(i))).unwrap();
        }

        for i in 0..10 {
            let ptr = buffer.pop().unwrap();
            unsafe {
                assert_eq!(*ptr, i, "FIFO order violated");
                drop_raw(ptr);
            }
        }

        assert!(buffer.pop().is_none());
    }

    #[test]
    fn test_push_full_returns_error() {
        let buffer = AtomicBuffer::<i32>::with_capacity(4);

        for i in 0..4 {
            buffer.push(Box::into_raw(Box::new(i))).unwrap();
        }

        let overflow = Box::into_raw(Box::new(999));
        let result = buffer.push(overflow);
        assert!(result.is_err());
        unsafe { drop_raw(result.unwrap_err()) };

        for ptr in buffer.drain_all() {
            unsafe { drop_raw(ptr) };
        }
    }

    // ==================== DRAIN_ALL ====================

    #[test]
    fn test_drain_all() {
        let buffer = AtomicBuffer::<i32>::new();

        // Empty
        assert!(buffer.drain_all().next().is_none());

        // With elements
        for i in 0..10 {
            buffer.push(Box::into_raw(Box::new(i))).unwrap();
        }

        let drained: Vec<_> = buffer.drain_all().collect();
        assert_eq!(drained.len(), 10);

        for ptr in drained {
            unsafe { drop_raw(ptr) };
        }

        assert!(buffer.pop().is_none());
    }

    // ==================== CLONE ====================

    #[test]
    fn test_clone_shares_data() {
        let buffer = AtomicBuffer::<i32>::new();
        buffer.push(Box::into_raw(Box::new(42))).unwrap();

        let c1 = buffer.clone();
        let c2 = buffer.clone();
        let c3 = c1.clone();

        // Pop from any clone affects all
        let ptr = c2.pop().unwrap();
        unsafe {
            assert_eq!(*ptr, 42);
            drop_raw(ptr);
        }

        assert!(buffer.pop().is_none());
        assert!(c1.pop().is_none());
        assert!(c3.pop().is_none());

        // Drop partial clones doesn't affect others
        drop(c1);
        drop(c2);

        buffer.push(Box::into_raw(Box::new(100))).unwrap();
        let ptr = c3.pop().unwrap();
        unsafe {
            assert_eq!(*ptr, 100);
            drop_raw(ptr);
        }
    }

    // ==================== MEMORY SAFETY ====================

    #[test]
    fn test_no_leaks_manual_cleanup() {
        let counter = Arc::new(AtomicUsize::new(0));

        let buffer = AtomicBuffer::new();
        for i in 0..16 {
            buffer
                .push(to_raw(Tracked::new(i, counter.clone())))
                .unwrap();
        }
        assert_eq!(counter.load(Ordering::SeqCst), 16);

        // Via pop
        for _ in 0..8 {
            unsafe { drop_raw(buffer.pop().unwrap()) };
        }
        assert_eq!(counter.load(Ordering::SeqCst), 8);

        // Via drain_all
        for ptr in buffer.drain_all() {
            unsafe { drop_raw(ptr) };
        }
        assert_eq!(counter.load(Ordering::SeqCst), 0);

        // Con cloni
        for i in 0..5 {
            buffer
                .push(to_raw(Tracked::new(i, counter.clone())))
                .unwrap();
        }
        let c1 = buffer.clone();
        let c2 = buffer.clone();
        drop(c1);
        drop(c2);
        assert_eq!(counter.load(Ordering::SeqCst), 5);

        for ptr in buffer.drain_all() {
            unsafe { drop_raw(ptr) };
        }
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    /// Verifica che AtomicBuffer::Drop liberi correttamente gli elementi rimasti
    #[test]
    fn test_drop_frees_remaining_elements() {
        let counter = Arc::new(AtomicUsize::new(0));

        {
            let buffer = AtomicBuffer::new();
            for i in 0..10 {
                buffer
                    .push(to_raw(Tracked::new(i, counter.clone())))
                    .unwrap();
            }
            assert_eq!(counter.load(Ordering::SeqCst), 10);
            // Buffer drops e libera gli elementi
        }

        assert_eq!(
            counter.load(Ordering::SeqCst),
            0,
            "Drop non ha liberato gli elementi"
        );
    }

    // ==================== EDGE CASES ====================

    #[test]
    fn test_wrap_around() {
        let buffer = AtomicBuffer::<i32>::with_capacity(4);

        for round in 0..5 {
            for i in 0..4 {
                buffer
                    .push(Box::into_raw(Box::new((round * 4 + i) as i32)))
                    .unwrap();
            }
            for _ in 0..4 {
                unsafe { drop_raw(buffer.pop().unwrap()) };
            }
        }

        assert!(buffer.pop().is_none());
    }

    #[test]
    fn test_push_pop_cycles() {
        let buffer = AtomicBuffer::<i32>::new();

        for cycle in 0..10 {
            for i in 0..5 {
                buffer
                    .push(Box::into_raw(Box::new(cycle * 10 + i)))
                    .unwrap();
            }
            for _ in 0..3 {
                unsafe { drop_raw(buffer.pop().unwrap()) };
            }
        }

        // 2 elementi rimasti per ciclo * 10 cicli = 20
        let mut remaining = 0;
        while let Some(ptr) = buffer.pop() {
            unsafe { drop_raw(ptr) };
            remaining += 1;
        }
        assert_eq!(remaining, 20);
    }

    #[test]
    fn test_capacity_one() {
        let buffer = AtomicBuffer::<i32>::with_capacity(1);

        let val = Box::into_raw(Box::new(42));
        assert!(buffer.push(val).is_ok());

        let overflow = Box::into_raw(Box::new(99));
        let err = buffer.push(overflow).unwrap_err();
        unsafe { drop_raw(err) };

        unsafe { drop_raw(buffer.pop().unwrap()) };
    }

    // ==================== CONCURRENCY ====================

    #[test]
    fn test_concurrent_producer_consumer() {
        let buffer = Arc::new(AtomicBuffer::<i32>::with_capacity(32));
        let items = 32;

        let prod_buffer = Arc::clone(&buffer);
        let producer = thread::spawn(move || {
            for i in 0..items {
                let val = Box::into_raw(Box::new(i as i32));
                while prod_buffer.push(val).is_err() {
                    thread::yield_now();
                }
            }
        });

        let cons_buffer = Arc::clone(&buffer);
        let consumer = thread::spawn(move || {
            let mut count = 0;
            while count < items {
                if let Some(ptr) = cons_buffer.pop() {
                    unsafe { drop_raw(ptr) };
                    count += 1;
                } else {
                    thread::yield_now();
                }
            }
        });

        producer.join().unwrap();
        consumer.join().unwrap();
        assert!(buffer.pop().is_none());
    }

    #[test]
    fn test_concurrent_multiple_producers() {
        let buffer = Arc::new(AtomicBuffer::<i32>::with_capacity(64));
        let barrier = Arc::new(Barrier::new(4));
        let items_per_thread = 10;

        let handles: Vec<_> = (0..4)
            .map(|t| {
                let buf = Arc::clone(&buffer);
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                    for i in 0..items_per_thread {
                        let val = Box::into_raw(Box::new((t * 100 + i) as i32));
                        while buf.push(val).is_err() {
                            thread::yield_now();
                        }
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        let drained: Vec<_> = buffer.drain_all().collect();
        assert_eq!(drained.len(), 4 * items_per_thread);
        for ptr in drained {
            unsafe { drop_raw(ptr) };
        }
    }

    #[test]
    fn test_concurrent_multiple_consumers() {
        let buffer = Arc::new(AtomicBuffer::<i32>::with_capacity(32));

        for i in 0..32 {
            buffer.push(Box::into_raw(Box::new(i))).unwrap();
        }

        let barrier = Arc::new(Barrier::new(4));
        let pop_count = Arc::new(AtomicUsize::new(0));

        let handles: Vec<_> = (0..4)
            .map(|_| {
                let buf = Arc::clone(&buffer);
                let b = barrier.clone();
                let count = pop_count.clone();
                thread::spawn(move || {
                    b.wait();
                    while let Some(ptr) = buf.pop() {
                        unsafe { drop_raw(ptr) };
                        count.fetch_add(1, Ordering::Relaxed);
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(pop_count.load(Ordering::SeqCst), 32);
    }

    #[test]
    fn test_concurrent_clone_drop() {
        let counter = Arc::new(AtomicUsize::new(0));
        let buffer = Arc::new(AtomicBuffer::new());

        for i in 0..8 {
            buffer
                .push(to_raw(Tracked::new(i, counter.clone())))
                .unwrap();
        }

        let barrier = Arc::new(Barrier::new(8));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let buf_clone = (*buffer).clone();
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                    thread::yield_now();
                    drop(buf_clone);
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        for ptr in buffer.drain_all() {
            unsafe { drop_raw(ptr) };
        }

        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_concurrent_mixed_operations() {
        let counter = Arc::new(AtomicUsize::new(0));
        let buffer = Arc::new(AtomicBuffer::with_capacity(32));
        let barrier = Arc::new(Barrier::new(6));

        // 2 producers
        let producers: Vec<_> = (0..2)
            .map(|t| {
                let buf = Arc::clone(&buffer);
                let c = counter.clone();
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                    for i in 0..20 {
                        let ptr = to_raw(Tracked::new(t * 20 + i, c.clone()));
                        while buf.push(ptr).is_err() {
                            thread::yield_now();
                        }
                    }
                })
            })
            .collect();

        // 2 consumers
        let consumed = Arc::new(AtomicUsize::new(0));
        let consumers: Vec<_> = (0..2)
            .map(|_| {
                let buf = Arc::clone(&buffer);
                let b = barrier.clone();
                let cons = consumed.clone();
                thread::spawn(move || {
                    b.wait();
                    loop {
                        if cons.load(Ordering::Relaxed) >= 40 {
                            break;
                        }
                        if let Some(ptr) = buf.pop() {
                            unsafe { drop_raw(ptr) };
                            cons.fetch_add(1, Ordering::Relaxed);
                        } else {
                            thread::yield_now();
                        }
                    }
                })
            })
            .collect();

        // 2 clone/droppers
        let cloners: Vec<_> = (0..2)
            .map(|_| {
                let buf = Arc::clone(&buffer);
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                    for _ in 0..5 {
                        let c = (*buf).clone();
                        thread::yield_now();
                        drop(c);
                    }
                })
            })
            .collect();

        for h in producers {
            h.join().unwrap();
        }
        for h in consumers {
            h.join().unwrap();
        }
        for h in cloners {
            h.join().unwrap();
        }

        for ptr in buffer.drain_all() {
            unsafe { drop_raw(ptr) };
        }

        assert_eq!(counter.load(Ordering::SeqCst), 0, "Memory leak detected");
    }
}
