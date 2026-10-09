# Blazingly Fast Concurrent Data Structures

`crossync` provides concurrent data structures and synchronization primitives for Rust.
It combines atomic operations, backoff strategies, and internal locks to support shared access in multithreaded applications.

- Concurrent collections for shared state, queues, and message passing
- Interior mutability with synchronized access
- Reference-counted handles for sharing supported containers without copying their contents

## Installation

Add `crossync` to your `Cargo.toml`:

```toml
[dependencies]
crossync = "0.2.1"
```

---

## AtomicVec

`AtomicVec<T>` is a thread-safe collection supporting concurrent push and pop operations.
It uses block-based allocation, atomic indices, and backoff strategies to manage storage and contention.

- Suitable for work queues and dynamic collections
- Shared and exclusive locking for access and reset operations
- Automatic block recycling and free-list management
- Conversion to a standard `Vec<T>` by consuming elements

### Example

```rust
use std::thread;
use crossync::atomic::AtomicVec;

let h = AtomicVec::new();

h.push("hello");
let b = h.clone();
drop(h);

{
    let b = b.clone();
    let t = thread::spawn(move || {
        if let Some(v) = b.pop() {
            assert_eq!(v, "hello");
        }
    });
    t.join().unwrap();
}

assert!(b.pop().is_none());
```

---

## AtomicHashMap

`AtomicHashMap<K, V>` is a thread-safe hash map that supports concurrent insertion, retrieval, and removal of key-value pairs.
It combines atomic operations with internal locks to coordinate access.

- Suitable for shared caches and application state
- Resizable bucket storage for growing collections

### Example

```rust
use std::thread;
use crossync::atomic::AtomicHashMap;

let h = AtomicHashMap::new();

h.insert("c", "hello");
let b = h.clone();
drop(h);

{
    let b = b.clone();
    let t = thread::spawn(move || {
        if let Some(mut v) = b.get_mut("c") {
            *v = "world";
        }
    });
    t.join().unwrap();
}

assert_eq!(b.get("c").unwrap(), "world");
```

---

## AtomicBuffer

`AtomicBuffer<T>` is a bounded, thread-safe ring buffer with per-slot sequence numbers.
It uses atomic push and pop operations for concurrent producers and consumers. A stalled reservation can delay other operations.

- Suitable for work queues, message passing, and object pools
- `push_box` and `pop_box` transfer ownership safely; raw `push` is unsafe and requires a uniquely owned, non-null `Box` allocation

### Example

```rust
use crossync::atomic::AtomicBuffer;
use std::thread;

let buffer = AtomicBuffer::with_capacity(2);

let producer = {
    let buffer = buffer.clone();
    thread::spawn(move || {
        buffer.push_box(Box::new(1)).unwrap();
        buffer.push_box(Box::new(2)).unwrap();
    })
};

let consumer = {
    let buffer = buffer.clone();
    thread::spawn(move || {
        let mut count = 1;
        while count <= 2 {
            if let Some(value) = buffer.pop_box() {
                assert_eq!(*value, count);
                count += 1;
            }
        }
    })
};

producer.join().unwrap();
consumer.join().unwrap();
```

---

## AtomicCell

`AtomicCell<T>` is a thread-safe, lock-assisted container for a single value.
It provides interior mutability through synchronized access and uses reference counting to share the value across cloned handles.

- Suitable for shared single-value state in multithreaded programs

### Example

```rust
use std::thread;
use crossync::atomic::AtomicCell;

let c = AtomicCell::new(10);
let c2 = c.clone();

let handle = thread::spawn(move || {
    let mut v = c2.get_mut();
    *v += 1;
});

handle.join().unwrap();

assert_eq!(*c.get(), 11);
```

---

## AtomicArray

`AtomicArray<T>` is a lock-assisted, thread-safe array supporting concurrent reads and writes.
It combines atomic indices and per-slot locks to coordinate access to stored values.

- Backoff strategies manage contention during concurrent operations

### Example

```rust
use std::thread;
use crossync::atomic::AtomicArray;

let arr = AtomicArray::with_capacity(4);
let arr_clone = arr.clone();

let t = thread::spawn(move || {
    let _ = arr_clone.push(10);
});

t.join().unwrap();

arr.for_each_mut(|v| {
    *v *= 2;
});

assert_eq!(*arr.get(0).unwrap(), 20);
```

---

## Atomic

`Atomic<T>` is a generic, lock-assisted container that provides synchronized access to a Rust value.
It supports primitives, structs, enums, collections, and other user-defined types. Sharing an `Atomic` container between threads requires `T: Send`, including types such as `Cell` and `RefCell` that are `!Sync`. The container manages locking internally.

- Load, store, swap, update, and compare-exchange operations, subject to method-specific trait bounds
- Specialized methods for `Vec<T>`, `String`, and `Option<T>`
- Numeric and bitwise operations such as `fetch_add` and `fetch_sub`

### Example

```rust
use crossync::atomic::Atomic;
use std::sync::Arc;
use std::thread;

#[derive(Debug, Clone, PartialEq)]
struct Person {
    name: String,
    age: u32,
}

let atomic = Arc::new(Atomic::new(Person {
    name: "Alice".to_string(),
    age: 30,
}));

let atomic2 = atomic.clone();
let handle = thread::spawn(move || {
    atomic2.update(|p| {
        p.name = "Bob".to_string();
        p.age += 1;
    });
});

handle.join().unwrap();

let result = atomic.load();
assert_eq!(result.name, "Bob");
assert_eq!(result.age, 31);
```

---

## Access and ownership

The following rules apply to the lock-assisted atomic containers:

- `Atomic`, `AtomicCell`, `AtomicArray`, and `AtomicHashMap` serialize access to the same value or map bucket, including read callbacks and `get()` guards.
- Nested reads on the acquiring thread are supported. Conflicting mutable reentrance and read-to-write upgrades panic before creating an alias.
- Release guards before conflicting access or array reset; acquire distinct locks in a consistent order.
- Read guards can transfer between threads when `T: Sync`; exclusive guards can transfer when `T: Send`. Reentrance checks use the acquiring-thread identity until the acquisition or read group is released.
- `AtomicHashMap::hasher()` returns a `Deref<Target = S>` guard. Use `map.hasher().build_hasher()` for method calls, `&*map.hasher()` for `&S` arguments, or `map.hasher_ref()` when `S: Sync`. Default constructors allow parallel access to their `Sync` builder.

See [CONCURRENCY.md](CONCURRENCY.md) for synchronization details and verification commands.

### Sharing a value with interior mutability

```rust
use crossync::atomic::AtomicCell;
use std::cell::Cell;
use std::thread;

let value = AtomicCell::new(Cell::new(0));
thread::scope(|scope| {
    for _ in 0..4 {
        let value = &value;
        scope.spawn(move || value.with(|cell| cell.set(cell.get() + 1)));
    }
});
assert_eq!(value.get().get(), 4);
```

---

## RwLock

`RwLock<T>` is a synchronization primitive that supports multiple readers or a single writer.
It uses atomic operations and platform-specific waiting mechanisms to coordinate access.

- Shared (read) and exclusive (write) locking modes
- Reference-counted cloning without copying the stored value
- No lock poisoning

### Example

```rust
use crossync::sync::RwLock;
use std::thread;
use std::thread::sleep;
use std::time::Duration;

let mutex = RwLock::new(5);

let m1 = mutex.clone();

let h1 = thread::spawn(move || {
    let _guard = m1.lock_exclusive();
    sleep(Duration::from_millis(10));
});

let m2 = mutex.clone();
let h2 = thread::spawn(move || {
    let _guard = m2.lock_shared();
    sleep(Duration::from_millis(10));
});

h1.join().unwrap();
h2.join().unwrap();
```

---

## Barrier

`Barrier` coordinates groups of threads by blocking waiters until the required number of arrivals is reached.
`Barrier::with_capacity(n, bucket)` requires `n` arrivals for the first phase and `bucket` arrivals for subsequent phases.
A zero `bucket` disables the barrier after the first phase; a zero `n` creates an already-disabled barrier.

- Suitable for parallel algorithms, phased execution, and workload synchronization

### Example

```rust
use crossync::sync::Barrier;
use std::thread;

let barrier = Barrier::with_capacity(3, 0);

let mut handles = vec![];
for _ in 0..3 {
    let c = barrier.clone();
    handles.push(thread::spawn(move || {
        println!("Waiting...");
        c.wait();
        println!("Released!");
    }));
}

for h in handles {
    h.join().unwrap();
}
```

---

## License

Licensed under the [Apache License 2.0](LICENSE).
