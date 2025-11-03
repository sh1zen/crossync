# 📦 Blazingly Fast Concurrent Data Structures

- 🪪 Thread-safe with spin-lock backoff and kernel-level mutexes
- ⚡ Optimized for high-concurrency workloads
- 💾 Optimized cloning with safe memory management via internal reference counting 
- 🔐 Internal mutability

---

## ✨ AtomicVec

**AtomicVec<T>** is a high-performance, thread-safe vector supporting concurrent push and pop operations with minimal locking overhead.
It uses block-based allocation, atomic indices, and internal backoff strategies to manage memory efficiently in multi-threaded contexts.

- 🧠 Suitable for implementing queues, stacks, and other dynamic collections
- 🛡️ Shared/exclusive locking for safe access and reset operations
- ♻️ Automatic block recycling and free-list management
- 📦 Can convert to standard Vec<T> safely, consuming elements

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

## ✨ AtomicHashMap

**AtomicHashMap** a blazingly fast thread-safe, concurrent hash map that supports high-performance insertion, retrieval, and removal of key-value pairs.  
It uses fine-grained atomic operations combined with internal mutexes to manage contention efficiently.

- 🧠 Ideal for shared caches, state maps, and in high concurrency scenario
- 📏 It uses resizable bucket array to optimize hash distribution and performance

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
            *v = "world"
        }
    });
    t.join().unwrap();
}

assert_eq!(b.get("c").unwrap(), "world");
```

---

## ✨ AtomicBuffer

**AtomicBuffer** is a lock-free, bounded, and thread-safe ring buffer.  
It provides atomic push and pop operations without requiring locks, making it ideal for high-performance concurrent producer/consumer systems.

- 🧠 Suitable for work queues, message passing, or object pooling systems

### Example

```rust
use crossync::atomic::AtomicBuffer;
use std::thread;

let buffer = AtomicBuffer::with_capacity(2);

let producer = {
    let buffer = buffer.clone();
    thread::spawn(move || {
        let _ = buffer.push(Box::into_raw(Box::new(1)));
        let _ = buffer.push(Box::into_raw(Box::new(2)));
    })
};

let consumer = {
    let buffer = buffer.clone();
    thread::spawn(move || {
        let mut count = 1;
        while count <= 2 {
            if let Some(ptr) = buffer.pop() {
                let val = unsafe { *Box::from_raw(ptr) };
                assert_eq!(val, count);
                count += 1;
            }
        }
    })
};

producer.join().unwrap();
consumer.join().unwrap();
```

---

## ✨ AtomicCell

**AtomicCell** is a thread-safe, lock-assisted atomic container that provides interior mutability with cloneable reference counting.  
It combines mutex-protected access, raw memory management, and atomic reference counting to safely store and manipulate a single value in concurrent environments.

- 🧠 Ideal for shared single-value state in multithreaded programs

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

## ✨ AtomicArray

**AtomicArray** is a lock-assisted, thread-safe array optimized for concurrent reads and writes.  
It combines atomic indices, per-slot locks, and cache-friendly memory layout to provide efficient and safe access in multi-threaded environments.

- 🧠 Optimized for high-concurrency workloads with backoff spins

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

## ✨ Atomic<T> — Universal Atomic Wrapper

**Atomic<T>** is a powerful generic atomic type providing thread-safe access to **any** Rust type `T`.  
It supports complex types, structs, enums, collections, primitives, and user-defined data — all synchronized via an internal `SMutex`.

- 🧠 Works with **any type**: primitives, structs, enums, strings, vectors, and custom types
- 🔄 Provides **atomic load, store, swap, update, and compare-exchange** operations
- 🧩 Specialized methods for common containers (`Vec<T>`, `String`, `Option<T>`)
- 🧮 Supports numeric and bitwise atomic operations (`fetch_add`, `fetch_sub`, etc.)
- 🔐 Thread-safe interior mutability with minimal overhead

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

## ✨ RwLock

**RwLock** is a lightweight, synchronization primitive for safe concurrent access. It provides multi-reader / single-writer locking with minimal kernel interaction.

 - ⚡ Fast atomic + futex-based design
 - 🔒 Shared (read) and exclusive (write) modes
 - 🧩 Clonable via internal ref-count (no data copy)
 - ✅ Compared to std::RwLock: user-space (faster, no poisoning, clonable).

### Example

```rust
use crossync::sync::RwLock;
use std::thread;
use std::thread::sleep;
use std::time::Duration;

let mutex = RwLock::new(5);

let m1 = mutex.clone();

let h1 = thread::spawn(move || {
let _guard = m1.lock();
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

## ✨ Barrier — Thread Synchronization Primitive

**Barrier** is a lightweight, thread-safe synchronization primitive that coordinates groups of threads.  
It blocks threads until a specified number of waiters arrive, then releases them all simultaneously.  
Once released, the barrier resets to a configurable capacity for reuse.

- 🧠 Suitable for parallel algorithms, phased execution, and workload synchronization

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

## 📦 Installation

Install `crossync` from crates.io  
Open your `Cargo.toml` and add:

```toml
[dependencies]
crossync = "0.0.3" # or the latest version available
```

---

## 📄 License

Apache-2.0
