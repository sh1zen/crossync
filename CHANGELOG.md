# Changelog

All notable changes to this project will be documented in this file.

## [0.2.1] - 2026-10-09

### Fixed

- Bounded concurrent AtomicArray reset test workloads to prevent integer overflow and prolonged contention in CI.
- Explicit stable Rust setup, dependency caching, timeouts and diagnostics in GitHub Actions.
- Documentation redirect to the crossync crate and scoped Pages permissions.
- Clean source release archives and explicit tag selection for manual releases.
- Deployment cleanup pagination and inactive status before deletion.

## [0.2.0] - 2026-10-09

### Fixed

- Thread-safety bounds for containers, guards and hash builders.
- AtomicVec block lifetimes, reclamation, bulk operations and partial consumption.
- AtomicBuffer reservations, ownership, draining and ring generations, including capacity one.
- Panic cleanup, array allocation checks and collection from imprecise iterators.
- Guard progress during array reset and map clear; value destruction outside locks.
- Futex waiter state, Linux arguments, wake-all counts and dedicated 32-bit words.
- SpinCell reader progress and Barrier arrival counts independent of clone count.

### API

- Atomic containers support `T: Send`, including `Cell` and `RefCell`, with serialized access.
- Nested reads are supported; conflicting mutable reentrance panics.
- Read guards transfer for `T: Sync`; exclusive guards transfer for `T: Send`.
- `AtomicHashMap::hasher()` returns a `Deref` guard; `hasher_ref()` requires `S: Sync`.
- `AtomicBuffer::push_box/pop_box` provide safe ownership transfer; raw `push` and SpinCell unlock methods are unsafe.
- `Atomic::replace_with` preserves the initialized value on callback panic.
- `Barrier::count` reports outstanding arrivals; zero initial capacity disables waiting.

### Performance and verification

- Inline per-slot locks and single-value atomic benchmarks.
- Regression tests for ownership, panic cleanup, reentrance and guard transfer.
- Documentation of access rules, ownership and verification commands.

## [0.1.2] - 2026-04-21

- improved performances
- fixed possible deadlocks

# [0.1.0] - 2026-01-13

- added some core utilities

## [0.0.4] - 2025-11-04

### Added 

- WatchGuardRef::downcast over Box<dyn Any>
- WatchGuardMut::downcast over Box<dyn Any>

## [0.0.2] - 2025-10-31

### Fixed 

 - futex issue on linux platforms
 - rearranged some code

## [0.0.1] - 2025-10-23

- Initial Release
