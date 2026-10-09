use crossync::atomic::{Atomic, AtomicArray, AtomicBuffer, AtomicCell, AtomicHashMap, AtomicVec};
use crossync::sync::{WatchGuardMut, WatchGuardRef};
use std::cell::{Cell, RefCell};
use std::hash::{BuildHasher, Hash, Hasher};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, Weak, mpsc};
use std::thread;
use std::time::Duration;

fn rounds() -> usize {
    if cfg!(miri) { 8 } else { 512 }
}
fn concurrent(f: impl Fn() + Sync) {
    thread::scope(|scope| {
        for _ in 0..4 {
            let f = &f;
            scope.spawn(move || {
                for _ in 0..rounds() {
                    f();
                }
            });
        }
    });
}

#[test]
fn send_without_sync_is_accepted_by_every_atomic_family() {
    fn send_sync<T: Send + Sync>() {}
    send_sync::<Atomic<Cell<usize>>>();
    send_sync::<AtomicCell<RefCell<Vec<usize>>>>();
    send_sync::<AtomicArray<Cell<usize>>>();
    send_sync::<AtomicHashMap<Key, Cell<usize>, Builder>>();
    send_sync::<AtomicVec<Cell<usize>>>();
    send_sync::<AtomicBuffer<Cell<usize>>>();
}

#[test]
fn atomic_read_callbacks_serialize_cell_mutation() {
    let atomic = Atomic::new(Cell::new(0usize));
    concurrent(|| atomic.with(|value| value.set(value.get() + 1)));
    assert_eq!(atomic.with(Cell::get), 4 * rounds());
    atomic.with(|value| atomic.with(|nested| assert!(std::ptr::eq(value, nested))));
    assert!(catch_unwind(AssertUnwindSafe(|| atomic.with(|_| atomic.update(|_| {})))).is_err());
    atomic.update(|value| value.set(7));
    assert_eq!(atomic.with(Cell::get), 7);
}

#[test]
fn cell_guards_preserve_nested_reads_and_serialize_between_threads() {
    let atomic = AtomicCell::new(Cell::new(0usize));
    let clone = atomic.clone();
    let first: WatchGuardRef<'_, Cell<usize>> = atomic.get();
    let second: WatchGuardRef<'_, Cell<usize>> = clone.get();
    first.set(1);
    assert_eq!(second.get(), 1);
    drop((first, second));
    concurrent(|| {
        let value = atomic.get();
        value.set(value.get() + 1);
    });
    assert_eq!(atomic.get().get(), 1 + 4 * rounds());
    assert!(
        catch_unwind(AssertUnwindSafe(|| atomic.with(|_| {
            let _ = atomic.get_mut();
        })))
        .is_err()
    );
    let mutable: WatchGuardMut<'_, Cell<usize>> = atomic.get_mut();
    mutable.set(9);
    drop(mutable);
    assert_eq!(atomic.get().get(), 9);
}

#[test]
fn refcell_borrow_panic_releases_the_atomic_lock() {
    let atomic = AtomicCell::new(RefCell::new(vec![1]));
    assert!(
        catch_unwind(AssertUnwindSafe(|| atomic.with(|value| {
            let _borrow = value.borrow_mut();
            atomic.with(|nested| {
                let _ = nested.borrow_mut();
            });
        })))
        .is_err()
    );
    concurrent(|| atomic.with(|value| value.borrow_mut().push(2)));
    assert_eq!(atomic.with(|value| value.borrow().len()), 1 + 4 * rounds());
}

#[test]
fn array_guards_and_callbacks_protect_cell_values() {
    let array: AtomicArray<_> = [Cell::new(0usize), Cell::new(10)].into_iter().collect();
    concurrent(|| array.with(0, |value| value.set(value.get() + 1)).unwrap());
    assert_eq!(array.get(0).unwrap().get(), 4 * rounds());
    let first: WatchGuardRef<'_, Cell<usize>> = array.get(0).unwrap();
    let second = array.get(0).unwrap();
    assert_eq!(first.get(), second.get());
    assert!(catch_unwind(AssertUnwindSafe(|| array.get_mut(0))).is_err());
    assert!(catch_unwind(AssertUnwindSafe(|| array.reset_with(1, || Cell::new(8)))).is_err());
    drop((first, second));
    concurrent(|| array.for_each(|value| value.set(value.get() + 1)));
    let snapshot = array.as_vec();
    assert_eq!(snapshot[0].get(), 8 * rounds());
    assert_eq!(snapshot[1].get(), 10 + 4 * rounds());
}

#[test]
fn array_reset_waits_without_blocking_the_guard_owners_other_reads() {
    let array: AtomicArray<_> = [Cell::new(1), Cell::new(2)].into_iter().collect();
    let (start_tx, start_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let guard = array.get(0).unwrap();
    thread::scope(|scope| {
        let array = &array;
        scope.spawn(move || {
            start_tx.send(()).unwrap();
            array.reset_with(2, || Cell::new(9)).unwrap();
            done_tx.send(()).unwrap();
        });
        start_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(done_rx.recv_timeout(Duration::from_millis(10)).is_err());
        assert_eq!(array.get(1).unwrap().get(), 2);
        assert_eq!(guard.get(), 1);
        drop(guard);
        done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    });
    assert_eq!(array.get(0).unwrap().get(), 9);
}

#[derive(Clone)]
struct Key {
    key: usize,
    comparisons: Cell<usize>,
}
impl Key {
    fn new(key: usize) -> Self {
        Self {
            key,
            comparisons: Cell::new(0),
        }
    }
}
impl Hash for Key {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.key.hash(h);
    }
}
impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        self.comparisons.set(self.comparisons.get() + 1);
        self.key == other.key
    }
}
impl Eq for Key {}
#[derive(Clone)]
struct Builder {
    builds: Cell<usize>,
}
struct Collision;
impl Hasher for Collision {
    fn write(&mut self, _: &[u8]) {}
    fn finish(&self) -> u64 {
        0
    }
}
impl BuildHasher for Builder {
    type Hasher = Collision;
    fn build_hasher(&self) -> Collision {
        self.builds.set(self.builds.get() + 1);
        Collision
    }
}

#[test]
fn map_serializes_non_sync_keys_values_and_hash_builder() {
    let map = AtomicHashMap::with_hasher(Builder {
        builds: Cell::new(0),
    });
    map.insert(Key::new(0), Cell::new(0usize));
    map.insert(Key::new(1), Cell::new(1));
    concurrent(|| {
        let key = Key::new(0);
        map.with(&key, |value| value.set(value.get() + 1)).unwrap();
        assert!(map.contains_key(&Key::new(1)));
    });
    assert_eq!(map.get(&Key::new(0)).unwrap().get(), 4 * rounds());
    let builder: WatchGuardRef<'_, Builder> = map.hasher();
    assert!(builder.builds.get() >= 8 * rounds());
    assert!(map.contains_key(&Key::new(0))); // reentrant read of the builder
    drop(builder);
    let snapshot: Vec<(Key, Cell<usize>)> = map.as_vec();
    assert_eq!(snapshot.len(), 2);
    assert!(snapshot.iter().any(|(key, _)| key.comparisons.get() > 0));
    assert!(
        catch_unwind(AssertUnwindSafe(|| map.with(&Key::new(0), |_| {
            map.remove(&Key::new(0));
        })))
        .is_err()
    );
    map.clear();
    assert!(map.is_empty());
}

#[test]
fn map_clear_destructors_can_read_the_map() {
    struct Value {
        owner: Weak<OnceLock<AtomicHashMap<usize, Value>>>,
        drops: Arc<AtomicUsize>,
    }
    impl Drop for Value {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::SeqCst);
            if let Some(owner) = self.owner.upgrade() {
                let map = owner.get().unwrap();
                assert!(!map.contains_key(&0));
                assert_eq!(map.len(), 0);
            }
        }
    }
    let owner = Arc::new(OnceLock::new());
    let drops = Arc::new(AtomicUsize::new(0));
    let map = AtomicHashMap::new();
    assert!(owner.set(map.clone()).is_ok());
    map.insert(
        0,
        Value {
            owner: Arc::downgrade(&owner),
            drops: drops.clone(),
        },
    );
    map.clear();
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[test]
fn transferred_sync_read_guards_cannot_race_last_release_and_reentry() {
    let array: AtomicArray<_> = [5usize].into_iter().collect();
    let first = array.get(0).unwrap();
    let second = array.get(0).unwrap();
    thread::scope(|scope| {
        scope.spawn(move || {
            assert_eq!(*first, 5);
            thread::yield_now();
            drop(first);
        });
        scope.spawn(move || {
            assert_eq!(*second, 5);
            thread::yield_now();
            drop(second);
        });
        for _ in 0..rounds() {
            assert_eq!(*array.get(0).unwrap(), 5);
        }
    });
    *array.get_mut(0).unwrap() = 6;
    assert_eq!(*array.get(0).unwrap(), 6);
    array.reset_with(2, || 7).unwrap();
}

#[test]
fn exclusive_cell_guard_can_transfer_ownership_to_another_thread() {
    let cell = AtomicCell::new(Cell::new(1));
    let guard = cell.get_mut();
    thread::scope(|scope| {
        scope.spawn(move || {
            guard.set(2);
        });
    });
    assert_eq!(cell.get().get(), 2);
}

#[test]
fn atomic_store_drops_replaced_values_after_unlocking() {
    #[derive(Clone)]
    struct Value {
        id: usize,
        owner: Weak<OnceLock<Atomic<Value>>>,
    }
    impl Drop for Value {
        fn drop(&mut self) {
            if self.id == 1 {
                if let Some(owner) = self.owner.upgrade() {
                    owner
                        .get()
                        .unwrap()
                        .with(|replacement| assert_eq!(replacement.id, 2));
                }
            }
        }
    }
    let owner = Arc::new(OnceLock::new());
    assert!(
        owner
            .set(Atomic::new(Value {
                id: 1,
                owner: Arc::downgrade(&owner)
            }))
            .is_ok()
    );
    owner.get().unwrap().store(Value {
        id: 2,
        owner: Arc::downgrade(&owner),
    });
    owner
        .get()
        .unwrap()
        .with(|replacement| assert_eq!(replacement.id, 2));
}

#[test]
fn map_clear_waits_without_closing_the_shard_reader_gate() {
    #[derive(Default)]
    struct Identity(u64);
    impl Hasher for Identity {
        fn write(&mut self, _: &[u8]) {
            unreachable!();
        }
        fn write_usize(&mut self, value: usize) {
            self.0 = value as u64;
        }
        fn finish(&self) -> u64 {
            self.0
        }
    }
    let map = AtomicHashMap::with_capacity_hasher_and_shard_amount(
        16,
        std::hash::BuildHasherDefault::<Identity>::default(),
        2,
    );
    map.insert(0usize, Cell::new(1));
    map.insert(1usize, Cell::new(2)); // same shard, different bucket
    let guard = map.get(&1).unwrap();
    let (start_tx, start_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    thread::scope(|scope| {
        let map = &map;
        scope.spawn(move || {
            start_tx.send(()).unwrap();
            map.clear();
            done_tx.send(()).unwrap();
        });
        start_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(done_rx.recv_timeout(Duration::from_millis(10)).is_err());
        // The earlier bucket must remain visible until the entire shard is ready.
        assert_eq!(map.get(&0).unwrap().get(), 1);
        drop(guard);
        done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    });
    assert!(map.is_empty());
}

#[test]
fn vector_reentrance_panics_before_data_alias_or_pin_leak() {
    let vector: AtomicVec<_> = [Cell::new(1)].into_iter().collect();
    assert!(
        catch_unwind(AssertUnwindSafe(
            || vector.for_each(|_| vector.push(Cell::new(2)))
        ))
        .is_err()
    );
    assert!(
        catch_unwind(AssertUnwindSafe(|| vector.for_each(|_| {
            vector.pop();
        })))
        .is_err()
    );
    vector.for_each(|value| value.set(3));
    assert_eq!(vector.pop().unwrap().get(), 3);
    vector.push(Cell::new(4));
    assert_eq!(vector.get(0).unwrap().get(), 4);
}
