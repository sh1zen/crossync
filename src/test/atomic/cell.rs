mod tests_atomic_cell {
    use crate::atomic::AtomicCell;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn test_basic_load_store() {
        let cell = AtomicCell::new(42);
        assert_eq!(*cell.get(), 42);

        cell.store(100);
        assert_eq!(*cell.get(), 100);
    }

    #[test]
    fn test_swap() {
        let cell = AtomicCell::new(10);
        let old = cell.swap(20);
        assert_eq!(old, 10);
        assert_eq!(*cell.get(), 20);
    }

    #[test]
    fn test_compare_exchange_success() {
        let cell = AtomicCell::new(5);
        let res = cell.compare_exchange(5, 10);
        assert_eq!(res.unwrap(), 5);
        assert_eq!(*cell.get(), 10);
    }

    #[test]
    fn test_compare_exchange_fail() {
        let cell = AtomicCell::new(5);
        let res = cell.compare_exchange(10, 20);
        assert!(res.is_err());
        drop(res);
        assert_eq!(*cell.get(), 5);
    }

    #[test]
    fn test_clone_and_drop_ref_count() {
        let cell = AtomicCell::new(42);
        let cloned = cell.clone();

        assert_eq!(*cell.get(), *cloned.get());

        drop(cell);
        // cloned should still be valid
        assert_eq!(*cloned.get(), 42);

        drop(cloned); // should free memory without leak
    }

    #[test]
    fn test_concurrent_access() {
        let cell = Arc::new(AtomicCell::new(0usize));
        let mut handles = vec![];

        for _ in 0..10 {
            let cell = Arc::clone(&cell);
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    cell.update(|v| *v += 1);
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        assert_eq!(*cell.get(), 1000);
    }

    #[test]
    fn test_reentrant_mut_access() {
        let cell = AtomicCell::new(vec![1, 2, 3]);

        {
            cell.get_mut().push(4);
        }
        {
            cell.get_mut().push(5);
        }

        let v = cell.get();
        assert_eq!(*v, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_complex_type() {
        #[derive(Debug, Clone, PartialEq)]
        struct Person {
            name: String,
            age: u32,
        }

        let cell = AtomicCell::new(Person {
            name: "Alice".into(),
            age: 30,
        });

        {
            let person = cell.get();
            assert_eq!(person.name, "Alice");
            assert_eq!(person.age, 30);
        }

        cell.store(Person {
            name: "Bob".into(),
            age: 25,
        });

        let person = cell.get();
        assert_eq!(person.name, "Bob");
        assert_eq!(person.age, 25);
    }

    #[test]
    fn test_concurrent_complex_type() {
        #[derive(Debug, Clone)]
        struct Task {
            id: u32,
            completed: bool,
        }

        let cell = Arc::new(AtomicCell::new(Task {
            id: 0,
            completed: false,
        }));

        let mut handles = vec![];
        for i in 0..5 {
            let cell = Arc::clone(&cell);
            handles.push(thread::spawn(move || {
                let mut guard = cell.get_mut();
                guard.id += 1;
                if i % 2 == 0 {
                    guard.completed = true;
                }
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        let task = cell.get();
        assert_eq!(task.id, 5);
    }

    #[test]
    fn test_into_inner() {
        let cell = AtomicCell::new(String::from("hello"));
        let value = cell.into_inner();
        assert_eq!(value, "hello");
    }

    #[test]
    fn test_into_inner_complex() {
        let cell = AtomicCell::new(vec![1, 2, 3, 4, 5]);
        let value = cell.into_inner();
        assert_eq!(value, vec![1, 2, 3, 4, 5]);
    }
}
