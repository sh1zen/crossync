mod tests_atomic_hashmap {
    use crate::atomic::AtomicHashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};
    use std::thread;

    // ==================== HELPERS ====================

    #[derive(Clone)]
    struct DropTracker {
        id: usize,
        counter: Arc<AtomicUsize>,
    }

    impl DropTracker {
        fn new(id: usize, counter: Arc<AtomicUsize>) -> Self {
            counter.fetch_add(1, Ordering::SeqCst);
            Self { id, counter }
        }
    }

    impl Drop for DropTracker {
        fn drop(&mut self) {
            self.counter.fetch_sub(1, Ordering::SeqCst);
        }
    }

    #[derive(Clone, PartialEq, Eq, Debug)]
    struct BadHash(i32);

    impl std::hash::Hash for BadHash {
        fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
            0i32.hash(state); // Sempre stesso hash -> collisioni
        }
    }

    // ==================== BASIC FUNCTIONALITY ====================

    #[test]
    fn test_insert_get_remove() {
        let map = AtomicHashMap::new();

        // Insert e get
        map.insert("key1", 10);
        map.insert("key2", 20);
        assert_eq!(*map.get("key1").unwrap(), 10);
        assert_eq!(*map.get("key2").unwrap(), 20);
        assert_eq!(map.len(), 2);

        // Overwrite
        map.insert("key1", 99);
        assert_eq!(*map.get("key1").unwrap(), 99);
        assert_eq!(map.len(), 2);

        // Remove
        assert_eq!(map.remove("key1"), Some(99));
        assert!(map.get("key1").is_none());
        assert_eq!(map.len(), 1);

        // Remove non-existent
        assert!(map.remove("nonexistent").is_none());
    }

    #[test]
    fn test_get_mut() {
        let map = AtomicHashMap::new();
        map.insert("x", vec![1, 2]);

        map.get_mut("x").unwrap().push(3);
        assert_eq!(*map.get("x").unwrap(), vec![1, 2, 3]);

        assert!(map.get_mut("nonexistent").is_none());
    }

    #[test]
    fn test_contains_and_iteration() {
        let map = AtomicHashMap::with_capacity(8);
        for i in 0..100 {
            map.insert(i, i * 10);
        }

        // contains_key
        assert!(map.contains_key(&50));
        assert!(!map.contains_key(&200));

        // as_vec
        let vec = map.as_vec::<i32, i32>();
        assert_eq!(vec.len(), 100);
        for (k, v) in &vec {
            assert_eq!(*v, *k * 10);
        }
    }

    #[test]
    fn test_empty_map() {
        let map: AtomicHashMap<i32, i32> = AtomicHashMap::new();

        assert!(map.get(&1).is_none());
        assert!(map.get_mut(&1).is_none());
        assert!(map.remove(&1).is_none());
        assert!(!map.contains_key(&1));
        assert_eq!(map.len(), 0);
        assert!(map.is_empty());

        map.clear(); // No-op su mappa vuota
    }

    #[test]
    fn test_clear() {
        let map = AtomicHashMap::new();
        for i in 0..100 {
            map.insert(i, i);
        }
        assert_eq!(map.len(), 100);

        map.clear();
        assert_eq!(map.len(), 0);
        assert!(map.is_empty());

        // Riuso dopo clear
        for i in 0..50 {
            map.insert(i, i * 2);
        }
        assert_eq!(map.len(), 50);
    }

    // ==================== MEMORY SAFETY ====================

    #[test]
    fn test_no_leaks() {
        let counter = Arc::new(AtomicUsize::new(0));

        {
            let map = AtomicHashMap::new();

            // Insert
            for i in 0..100 {
                map.insert(i, DropTracker::new(i, counter.clone()));
            }
            assert_eq!(counter.load(Ordering::SeqCst), 100);

            // Overwrite (vecchi valori devono essere droppati)
            for i in 0..50 {
                map.insert(i, DropTracker::new(i + 1000, counter.clone()));
            }
            assert_eq!(counter.load(Ordering::SeqCst), 100, "Overwrite leak");

            // Remove
            for i in 0..25 {
                map.remove(&i);
            }
            assert_eq!(counter.load(Ordering::SeqCst), 75, "Remove leak");

            // Clear
            map.clear();
            assert_eq!(counter.load(Ordering::SeqCst), 0, "Clear leak");

            // Reinserimento dopo clear
            for i in 0..50 {
                map.insert(i, DropTracker::new(i, counter.clone()));
            }
            assert_eq!(counter.load(Ordering::SeqCst), 50);
        }

        assert_eq!(counter.load(Ordering::SeqCst), 0, "Drop leak");
    }

    #[test]
    fn test_clone_refcount() {
        let counter = Arc::new(AtomicUsize::new(0));

        {
            let map = AtomicHashMap::new();
            for i in 0..50 {
                map.insert(i, DropTracker::new(i, counter.clone()));
            }

            // Cloni condividono i dati
            let c1 = map.clone();
            let c2 = map.clone();
            assert_eq!(counter.load(Ordering::SeqCst), 50);

            // Mutazioni visibili attraverso i cloni
            c1.insert(100, DropTracker::new(100, counter.clone()));
            assert!(c2.contains_key(&100));

            // Drop parziale non libera i dati
            drop(c1);
            drop(c2);
            assert_eq!(counter.load(Ordering::SeqCst), 51);
        }

        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    // ==================== HASH COLLISIONS ====================

    #[test]
    fn test_hash_collisions() {
        let counter = Arc::new(AtomicUsize::new(0));

        {
            let map = AtomicHashMap::new();

            // Tutte le chiavi hanno stesso hash
            for i in 0..100 {
                map.insert(BadHash(i), DropTracker::new(i as usize, counter.clone()));
            }
            assert_eq!(map.len(), 100);
            assert_eq!(counter.load(Ordering::SeqCst), 100);

            // Get funziona con collisioni
            for i in 0..100 {
                assert!(map.get(&BadHash(i)).is_some());
            }

            // Remove con collisioni
            for i in (0..100).step_by(2) {
                map.remove(&BadHash(i));
            }
            assert_eq!(map.len(), 50);
            assert_eq!(counter.load(Ordering::SeqCst), 50);
        }

        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    // ==================== EDGE CASES ====================

    #[test]
    #[cfg_attr(miri, ignore)]
    fn test_large_values() {
        let map = AtomicHashMap::new();
        let large_data: Vec<u8> = vec![0xAB; 1024 * 1024]; // 1MB

        for i in 0..5 {
            map.insert(i, large_data.clone());
        }

        for i in 0..5 {
            let v = map.get(&i).unwrap();
            assert_eq!(v.len(), 1024 * 1024);
            assert!(v.iter().all(|&b| b == 0xAB));
        }
    }

    #[test]
    fn test_zst_values() {
        let counter = Arc::new(AtomicUsize::new(0));

        struct ZST(Arc<AtomicUsize>);
        impl Drop for ZST {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::SeqCst);
            }
        }

        {
            let map = AtomicHashMap::new();
            for i in 0..100 {
                counter.fetch_add(1, Ordering::SeqCst);
                map.insert(i, ZST(counter.clone()));
            }
            assert_eq!(counter.load(Ordering::SeqCst), 100);
        }

        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    // ==================== CONCURRENCY ====================

    #[test]
    fn test_concurrent_get_mut_counter() {
        let map = Arc::new(AtomicHashMap::new());
        map.insert(0, 0i64);

        let barrier = Arc::new(Barrier::new(10));
        let handles: Vec<_> = (0..10)
            .map(|_| {
                let map = map.clone();
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                    for _ in 0..1000 {
                        if let Some(mut v) = map.get_mut(&0) {
                            *v += 1;
                        }
                    }
                })
            })
            .collect();

        for h in handles { h.join().unwrap(); }

        assert_eq!(*map.get(&0).unwrap(), 10_000, "Data race detected");
    }

    #[test]
    fn test_concurrent_mixed_ops() {
        let map = Arc::new(AtomicHashMap::new());
        let barrier = Arc::new(Barrier::new(12));

        // 4 inseritori (0..50)
        let inserters: Vec<_> = (0..4).map(|t| {
            let map = map.clone();
            let b = barrier.clone();
            thread::spawn(move || {
                b.wait();
                for i in 0..50 {
                    map.insert(i, format!("{}_{}", t, i));
                }
            })
        }).collect();

        // 4 lettori
        let readers: Vec<_> = (0..4).map(|_| {
            let map = map.clone();
            let b = barrier.clone();
            thread::spawn(move || {
                b.wait();
                for _ in 0..100 {
                    for i in 0..50 {
                        let _ = map.get(&i);
                    }
                }
            })
        }).collect();

        // 4 rimovitori
        let removers: Vec<_> = (0..4).map(|_| {
            let map = map.clone();
            let b = barrier.clone();
            thread::spawn(move || {
                b.wait();
                for _ in 0..20 {
                    for i in 0..50 {
                        map.remove(&i);
                    }
                }
            })
        }).collect();

        for h in inserters { h.join().unwrap(); }
        for h in readers { h.join().unwrap(); }
        for h in removers { h.join().unwrap(); }
    }

    #[test]
    fn test_concurrent_clone_drop() {
        let counter = Arc::new(AtomicUsize::new(0));
        let map = Arc::new(AtomicHashMap::new());

        for i in 0..50 {
            map.insert(i, DropTracker::new(i, counter.clone()));
        }

        let barrier = Arc::new(Barrier::new(10));
        let handles: Vec<_> = (0..10)
            .map(|_| {
                let map_clone = (*map).clone();
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                    // Operazioni sul clone
                    for i in 0..50 {
                        let _ = map_clone.get(&i);
                    }
                    drop(map_clone);
                })
            })
            .collect();

        for h in handles { h.join().unwrap(); }
        drop(map);

        assert_eq!(counter.load(Ordering::SeqCst), 0, "Concurrent clone/drop leak");
    }

    #[test]
    fn test_concurrent_collisions() {
        let map = Arc::new(AtomicHashMap::new());
        let barrier = Arc::new(Barrier::new(10));

        let handles: Vec<_> = (0..10)
            .map(|t| {
                let map = map.clone();
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                    for i in 0..50 {
                        let key = BadHash(t * 50 + i);
                        map.insert(key.clone(), i);
                        let _ = map.get(&key);
                        map.remove(&key);
                        map.insert(key, i * 2);
                    }
                })
            })
            .collect();

        for h in handles { h.join().unwrap(); }
    }

    #[test]
    fn test_guard_validity_under_mutation() {
        let map = Arc::new(AtomicHashMap::new());
        for i in 0..100 {
            map.insert(i, format!("value_{}", i));
        }

        let barrier = Arc::new(Barrier::new(2));

        let map1 = map.clone();
        let b1 = barrier.clone();
        let reader = thread::spawn(move || {
            b1.wait();
            for _ in 0..50 {
                for i in 0..100 {
                    if let Some(v) = map1.get(&i) {
                        assert!(v.starts_with("value_"), "Data corruption");
                    }
                }
            }
        });

        let map2 = map.clone();
        let b2 = barrier.clone();
        let mutator = thread::spawn(move || {
            b2.wait();
            for _ in 0..25 {
                for i in 0..100 {
                    map2.remove(&i);
                    map2.insert(i, format!("value_{}", i));
                }
            }
        });

        reader.join().unwrap();
        mutator.join().unwrap();
    }
}