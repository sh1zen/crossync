mod tests_atomic_wrapper {
    use crate::atomic::Atomic;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn test_custom_struct_load_store() {
        #[derive(Debug, Clone, PartialEq)]
        struct Person {
            name: String,
            age: u32,
        }

        let atomic = Atomic::new(Person {
            name: "Alice".into(),
            age: 30,
        });

        assert_eq!(atomic.load().name, "Alice");

        atomic.store(Person {
            name: "Bob".into(),
            age: 25,
        });

        assert_eq!(atomic.load().age, 25);
    }

    #[test]
    fn test_enum_usage() {
        #[derive(Debug, Clone, PartialEq)]
        enum State {
            Idle,
            Running(u32),
            Stopped,
        }

        let atomic = Atomic::new(State::Idle);
        atomic.store(State::Running(10));
        assert_eq!(atomic.load(), State::Running(10));

        atomic.update(|s| *s = State::Stopped);
        assert_eq!(atomic.load(), State::Stopped);
    }

    // -----------------------------
    // Closure-based access
    // -----------------------------

    #[test]
    fn test_with_and_with_mut() {
        #[derive(Clone)]
        struct Counter {
            count: u32,
        }

        let atomic = Atomic::new(Counter { count: 0 });

        let c = atomic.with(|v| v.count);
        assert_eq!(c, 0);

        atomic.with_mut(|v| v.count += 5);
        assert_eq!(atomic.with(|v| v.count), 5);
    }

    // -----------------------------
    // String / Vec / Option helpers
    // -----------------------------

    #[test]
    fn test_string_ops() {
        let atomic = Atomic::new(String::from("hi"));

        atomic.push_str(" there");
        assert_eq!(atomic.load(), "hi there");

        atomic.clear();
        assert!(atomic.is_empty());
    }

    #[test]
    fn test_vec_ops() {
        let atomic = Atomic::new(Vec::<i32>::new());

        atomic.push(1);
        atomic.push(2);
        assert_eq!(atomic.len(), 2);

        assert_eq!(atomic.pop(), Some(2));
        assert_eq!(atomic.len(), 1);
    }

    #[test]
    fn test_option_ops() {
        let atomic = Atomic::new(Some(10));

        assert!(atomic.is_some());
        assert_eq!(atomic.take(), Some(10));
        assert!(atomic.is_none());

        assert_eq!(atomic.replace(20), None);
        assert_eq!(atomic.load(), Some(20));
    }

    // -----------------------------
    // Drop correctness / leak detection
    // -----------------------------

    #[test]
    fn test_drop_exactly_once_replace_with() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        static DROPS: AtomicUsize = AtomicUsize::new(0);

        #[derive(Clone)]
        struct DropCounter;

        impl Drop for DropCounter {
            fn drop(&mut self) {
                DROPS.fetch_add(1, Ordering::SeqCst);
            }
        }

        {
            let atomic = Atomic::new(DropCounter);
            let _old = atomic.replace_with(|_| DropCounter);
        }

        assert_eq!(DROPS.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn test_rc_not_leaked() {
        use std::rc::Rc;

        let weak = {
            let rc = Rc::new(());
            let weak = Rc::downgrade(&rc);

            {
                let atomic = Atomic::new(rc.clone());
                atomic.store(Rc::new(()));
            }

            // Drop the last strong reference
            drop(rc);

            weak
        };

        assert!(weak.upgrade().is_none());
    }

    // -----------------------------
    // Panic safety
    // -----------------------------

    #[test]
    fn test_replace_with_safe() {
        #[derive(Clone, PartialEq, Debug)]
        struct Data {
            value: i32,
        }

        let atomic = Atomic::new(Data { value: 10 });

        let old = atomic.replace_with(|d| Data { value: d.value * 2 });

        assert_eq!(old.value, 10);
        assert_eq!(atomic.load().value, 20);
    }

    #[test]
    fn test_update_safe() {
        let atomic = Atomic::new(vec![1, 2, 3]);

        // Simula un “fallimento” senza panic
        let result: Result<(), ()> = atomic.try_update(|v| {
            if v.len() > 0 {
                v.push(4);
                false // simula "non successo"
            } else {
                true
            }
        });

        assert!(result.is_err());
        let v = atomic.load();
        assert_eq!(v, vec![1, 2, 3, 4]); // modificato in-place, ma nessun panic
    }

    // -----------------------------
    // Compare-and-swap semantics
    // -----------------------------

    #[test]
    fn test_compare_exchange() {
        let atomic = Atomic::new(0);

        assert_eq!(atomic.compare_exchange(0, 1), Ok(0));
        assert_eq!(atomic.compare_exchange(0, 2), Err(1));
        assert_eq!(atomic.load(), 1);
    }

    // -----------------------------
    // High-contention concurrency
    // -----------------------------

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_heavy_contention_fetch_add() {
        let atomic = Arc::new(Atomic::new(0usize));
        let threads = 8;
        let iterations = 100_000;

        let mut handles = Vec::new();

        for _ in 0..threads {
            let a = Arc::clone(&atomic);
            handles.push(thread::spawn(move || {
                for _ in 0..iterations {
                    a.fetch_add(1);
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(atomic.load(), threads * iterations);
    }

    #[test]
    fn test_concurrent_complex_type() {
        #[derive(Clone)]
        struct Task {
            count: usize,
            data: Vec<String>,
        }

        let atomic = Arc::new(Atomic::new(Task {
            count: 0,
            data: Vec::new(),
        }));

        let mut handles = Vec::new();

        for i in 0..5 {
            let a = Arc::clone(&atomic);
            handles.push(thread::spawn(move || {
                a.with_mut(|t| {
                    t.count += 1;
                    t.data.push(format!("t{}", i));
                });
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        let task = atomic.load();
        assert_eq!(task.count, 5);
        assert_eq!(task.data.len(), 5);
    }
}
