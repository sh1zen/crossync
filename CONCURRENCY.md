# Atomic access, ownership and verification

The Atomic families accept shared values with `T: Send`, including `Cell` and
`RefCell`. `AtomicHashMap` also supports `Send` keys and hash builders that are
`!Sync`. Synchronization is internal; callers do not need to wrap the stored
value in another lock.

## Safety and ownership

`SerialMutex` keeps the acquiring thread, reader count, writer flag and waiter
registration in one atomic word. Read aliases belong to one acquiring thread;
other threads wait. Writers exclude every read guard. The guard's acquisition
token lets an uncontended last reader release with one CAS; contention falls
back to a counted release that preserves waiter registration.

Read guards are `Send` and `Sync` only for `T: Sync`. Exclusive guards are
`Send` for `T: Send` and `Sync` only for `T: Sync`. These restrictions keep
`Cell` read aliases on one thread while allowing ownership of a mutable guard
to transfer. Container `Send`/`Sync` implementations rely on these locks and
guard restrictions, rather than assuming that `Send` values permit concurrent
shared references.

Array guards pin storage until slot unlock and its wake have finished. Reset
uses nonblocking attempts at exclusive coordination, so waiting for a guard
does not prevent its owner from finishing another read. Map clear acquires all
bucket locks before detaching a shard, releases failed attempts before retrying,
and destroys detached values outside locks. This preserves removal per shard.
`Atomic::store` and `AtomicCell::store` also destroy replaced values after
releasing the lock.

Futex words are separate 32-bit atomics. Rust and operating-system accesses
therefore use the same width, as required by the
[Rust atomic memory model](https://doc.rust-lang.org/std/sync/atomic/index.html#memory-model-for-atomic-accesses).

## Access rules

- Nested reads on the acquiring thread are supported; conflicting mutable
  reentrance and read-to-write upgrades panic before exposing an alias.
- Release guards before conflicting access or array reset. Reentrance checks
  use the acquiring-thread identity, including after guard transfer.
- Acquire distinct locks in a consistent order.
- `AtomicHashMap::hasher()` returns a `Deref<Target = S>` guard. Use
  `map.hasher().build_hasher()` for method calls, `&*map.hasher()` for `&S`
  arguments, or `map.hasher_ref()` when `S: Sync`.
- Default map constructors allow parallel access to their `Sync` builder;
  generic builders are protected internally.
- `AtomicBuffer::push_box/pop_box` transfer ownership safely. Raw `push`
  requires a uniquely owned, non-null `Box`-compatible allocation.
- SpinCell raw unlock methods are unsafe; guards manage unlocking automatically.
- `Barrier::count()` reports outstanding arrivals. `with_capacity(n, bucket)`
  uses `n` arrivals for the first phase and `bucket` for subsequent phases;
  zero disables the corresponding phase. Sharing through `Arc` is supported.

## Verification

Run unit tests, integration tests, documentation examples and benchmarks from
the repository root:

```text
cargo test --lib --tests
cargo test --doc
cargo test --release --test send_access
cargo bench --bench all
```

The integration suite in `tests/send_access.rs` covers interior mutation,
guard transfer, reentrance, panic cleanup and waiter progress.

## Performance

Local release measurements use rustc 1.99.0 and seven runs without concurrent
test or build jobs. Times are observed medians; single-value cases perform one
million operations. Throughput depends on workload and contention, including
the duration of serialized read callbacks on the same value.

| Case | Median time |
|---|---:|
| Atomic load_copy | 10.18 ms |
| Atomic with, 4 threads | 13.35 ms |
| AtomicCell get | 10.22 ms |
| AtomicCell get, 4 threads | 11.60 ms |
| AtomicCell get_mut | 11.35 ms |
| AtomicCell get_mut, 4 threads | 11.76 ms |
| AtomicCell store | 11.32 ms |
| AtomicArray get | 21.44 ms |
| AtomicArray get_mut | 22.72 ms |
| AtomicVec push/pop | 43.71 ms |
| AtomicVec push/pop, 24 threads | 49.45 ms |
| HashMap mixed, 1 thread | 2.837 ms |
| HashMap mixed, 16 threads | 16.288 ms |

Inline per-slot locks keep lock storage within the container allocation.
A counting allocator measured requested heap bytes for `usize` containers;
all measured allocations were released after drop:

| Container | Heap bytes | Allocations |
|---|---:|---:|
| Atomic | 0 | 0 |
| AtomicCell | 256 | 1 |
| AtomicArray, 10,000 slots | 321,152 | 3 |
| AtomicHashMap, default | 197,376 | 194 |

Single-value benchmarks are in `benches/bench_atomic.rs`.
