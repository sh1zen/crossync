use std::cell::UnsafeCell;
use std::fmt;
use std::ops::{Add, Sub, BitAnd, BitOr, BitXor};
use crate::core::smutex::SMutex;

/// A generic atomic type that provides thread-safe access to **ANY** type `T`.
/// Uses `SMutex` internally for synchronization.
///
/// Unlike std atomics which only work with primitive types, this works with:
/// - Primitives (bool, integers, floats)
/// - Structs and enums
/// - Strings and collections
/// - Any custom type
pub struct Atomic<T> {
    mutex: SMutex,
    value: UnsafeCell<T>,
}

// Safety: Atomic<T> can be shared across threads if T is Send
unsafe impl<T: Send> Sync for Atomic<T> {}
unsafe impl<T: Send> Send for Atomic<T> {}

/// Core operations - work with ANY type T
impl<T> Atomic<T> {
    /// Creates a new atomic value.
    pub fn new(value: T) -> Self {
        Self {
            mutex: SMutex::new(),
            value: UnsafeCell::new(value),
        }
    }

    /// Consumes the atomic and returns the contained value.
    pub fn into_inner(self) -> T {
        self.value.into_inner()
    }

    /// Returns a mutable reference to the underlying value.
    /// This is safe because it requires a mutable reference to self.
    pub fn get_mut(&mut self) -> &mut T {
        self.value.get_mut()
    }

    /// Applies a function to the value and returns the result.
    /// The function receives an immutable reference to the value.
    pub fn with<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        let _guard = self.mutex.lock_group();
        unsafe { f(&*self.value.get()) }
    }

    /// Applies a function to the value mutably.
    /// The function receives a mutable reference to modify the value.
    pub fn with_mut<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut T) -> R,
    {
        let _guard = self.mutex.lock();
        unsafe { f(&mut *self.value.get()) }
    }

    /// Replaces the value with a new one computed from a closure.
    /// Returns the old value.
    pub fn replace_with<F>(&self, f: F) -> T
    where
        F: FnOnce(&T) -> T,
    {
        let _guard = self.mutex.lock();
        unsafe {
            let old = std::ptr::read(self.value.get());
            let new = f(&old);
            std::ptr::write(self.value.get(), new);
            old
        }
    }

    /// Updates the value using a closure.
    /// The closure receives a mutable reference and can modify in-place.
    pub fn update<F>(&self, f: F)
    where
        F: FnOnce(&mut T),
    {
        let _guard = self.mutex.lock();
        unsafe { f(&mut *self.value.get()) }
    }

    /// Tries to update the value using a closure.
    /// Returns Ok(()) if the closure returns true, Err(()) otherwise.
    pub fn try_update<F>(&self, f: F) -> Result<(), ()>
    where
        F: FnOnce(&mut T) -> bool,
    {
        let _guard = self.mutex.lock();
        unsafe {
            if f(&mut *self.value.get()) {
                Ok(())
            } else {
                Err(())
            }
        }
    }
}

/// Operations for Clone types
impl<T: Clone> Atomic<T> {
    /// Loads a clone of the value from the atomic.
    pub fn load(&self) -> T {
        let _guard = self.mutex.lock_group();
        unsafe { (*self.value.get()).clone() }
    }

    /// Stores a value into the atomic.
    pub fn store(&self, val: T) {
        let _guard = self.mutex.lock();
        unsafe {
            *self.value.get() = val;
        }
    }

    /// Swaps the value, returning the previous value.
    pub fn swap(&self, val: T) -> T {
        let _guard = self.mutex.lock();
        unsafe {
            let old = (*self.value.get()).clone();
            *self.value.get() = val;
            old
        }
    }

    /// Takes the value, replacing it with a default.
    pub fn take_default(&self) -> T
    where
        T: Default,
    {
        self.swap(T::default())
    }
}

/// Operations for Copy types (optimized, no cloning)
impl<T: Copy> Atomic<T> {
    /// Loads a copy of the value (optimized for Copy types).
    pub fn load_copy(&self) -> T {
        let _guard = self.mutex.lock_group();
        unsafe { *self.value.get() }
    }

    /// Swaps the value, returning the previous value (optimized for Copy types).
    pub fn swap_copy(&self, val: T) -> T {
        let _guard = self.mutex.lock();
        unsafe {
            let old = *self.value.get();
            *self.value.get() = val;
            old
        }
    }
}

/// Compare and exchange operations
impl<T: Clone + PartialEq> Atomic<T> {
    /// Stores a value if the current value is the same as `current`.
    /// Returns `Ok(old_value)` on success, `Err(current_value)` on failure.
    pub fn compare_exchange(&self, current: T, new: T) -> Result<T, T> {
        let _guard = self.mutex.lock();
        unsafe {
            let val = (*self.value.get()).clone();
            if val == current {
                *self.value.get() = new;
                Ok(val)
            } else {
                Err(val)
            }
        }
    }

    /// Weak version of compare_exchange (same as strong for sync-based impl).
    pub fn compare_exchange_weak(&self, current: T, new: T) -> Result<T, T> {
        self.compare_exchange(current, new)
    }

    /// Compares and sets the value, returning the previous value.
    pub fn compare_and_swap(&self, current: T, new: T) -> T {
        match self.compare_exchange(current, new) {
            Ok(val) | Err(val) => val,
        }
    }

    /// Fetches the value and updates it with a function.
    pub fn fetch_update<F>(&self, mut f: F) -> Result<T, T>
    where
        F: FnMut(&T) -> Option<T>,
    {
        let _guard = self.mutex.lock();
        unsafe {
            let old = (*self.value.get()).clone();
            if let Some(new) = f(&old) {
                *self.value.get() = new;
                Ok(old)
            } else {
                Err(old)
            }
        }
    }
}

// Numeric operations for Copy types
impl<T: Copy + Add<Output = T>> Atomic<T> {
    /// Adds to the current value, returning the previous value.
    pub fn fetch_add(&self, val: T) -> T {
        let _guard = self.mutex.lock();
        unsafe {
            let old = *self.value.get();
            *self.value.get() = old + val;
            old
        }
    }
}

impl<T: Copy + Sub<Output = T>> Atomic<T> {
    /// Subtracts from the current value, returning the previous value.
    pub fn fetch_sub(&self, val: T) -> T {
        let _guard = self.mutex.lock();
        unsafe {
            let old = *self.value.get();
            *self.value.get() = old - val;
            old
        }
    }
}

// Bitwise operations for Copy types (excluding bool to avoid conflicts)
impl<T: Copy + BitAnd<Output = T>> Atomic<T> {
    /// Performs bitwise AND, returning the previous value.
    pub fn fetch_and_bits(&self, val: T) -> T {
        let _guard = self.mutex.lock();
        unsafe {
            let old = *self.value.get();
            *self.value.get() = old & val;
            old
        }
    }
}

impl<T: Copy + BitOr<Output = T>> Atomic<T> {
    /// Performs bitwise OR, returning the previous value.
    pub fn fetch_or_bits(&self, val: T) -> T {
        let _guard = self.mutex.lock();
        unsafe {
            let old = *self.value.get();
            *self.value.get() = old | val;
            old
        }
    }
}

impl<T: Copy + BitXor<Output = T>> Atomic<T> {
    /// Performs bitwise XOR, returning the previous value.
    pub fn fetch_xor_bits(&self, val: T) -> T {
        let _guard = self.mutex.lock();
        unsafe {
            let old = *self.value.get();
            *self.value.get() = old ^ val;
            old
        }
    }
}

impl<T: Copy + PartialOrd> Atomic<T> {
    /// Stores the maximum of the current value and `val`, returning the previous value.
    pub fn fetch_max(&self, val: T) -> T {
        let _guard = self.mutex.lock();
        unsafe {
            let old = *self.value.get();
            if val > old {
                *self.value.get() = val;
            }
            old
        }
    }

    /// Stores the minimum of the current value and `val`, returning the previous value.
    pub fn fetch_min(&self, val: T) -> T {
        let _guard = self.mutex.lock();
        unsafe {
            let old = *self.value.get();
            if val < old {
                *self.value.get() = val;
            }
            old
        }
    }
}

// Boolean-specific operations
impl Atomic<bool> {
    /// Performs logical AND, returning the previous value.
    pub fn fetch_and(&self, val: bool) -> bool {
        let _guard = self.mutex.lock();
        unsafe {
            let old = *self.value.get();
            *self.value.get() = old && val;
            old
        }
    }

    /// Performs logical OR, returning the previous value.
    pub fn fetch_or(&self, val: bool) -> bool {
        let _guard = self.mutex.lock();
        unsafe {
            let old = *self.value.get();
            *self.value.get() = old || val;
            old
        }
    }

    /// Performs logical XOR, returning the previous value.
    pub fn fetch_xor(&self, val: bool) -> bool {
        let _guard = self.mutex.lock();
        unsafe {
            let old = *self.value.get();
            *self.value.get() = old != val;
            old
        }
    }

    /// Performs logical NAND, returning the previous value.
    pub fn fetch_nand(&self, val: bool) -> bool {
        let _guard = self.mutex.lock();
        unsafe {
            let old = *self.value.get();
            *self.value.get() = !(old && val);
            old
        }
    }
}

// Integer-specific nand operation
macro_rules! impl_nand {
    ($($t:ty),*) => {
        $(
            impl Atomic<$t> {
                /// Performs bitwise NAND, returning the previous value.
                pub fn fetch_nand(&self, val: $t) -> $t {
                    let _guard = self.mutex.lock();
                    unsafe {
                        let old = *self.value.get();
                        *self.value.get() = !(old & val);
                        old
                    }
                }
            }
        )*
    };
}

impl_nand!(i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize);

// String-specific operations
impl Atomic<String> {
    /// Appends a string slice to the atomic string.
    pub fn push_str(&self, s: &str) {
        self.update(|string| string.push_str(s));
    }

    /// Clears the string.
    pub fn clear(&self) {
        self.update(|string| string.clear());
    }

    /// Returns the length of the string.
    pub fn len(&self) -> usize {
        self.with(|string| string.len())
    }

    /// Returns true if the string is empty.
    pub fn is_empty(&self) -> bool {
        self.with(|string| string.is_empty())
    }
}

// Vec-specific operations
impl<T: Clone> Atomic<Vec<T>> {
    /// Pushes an element to the vector.
    pub fn push(&self, value: T) {
        self.update(|vec| vec.push(value));
    }

    /// Pops an element from the vector.
    pub fn pop(&self) -> Option<T> {
        self.with_mut(|vec| vec.pop())
    }

    /// Returns the length of the vector.
    pub fn len(&self) -> usize {
        self.with(|vec| vec.len())
    }

    /// Returns true if the vector is empty.
    pub fn is_empty(&self) -> bool {
        self.with(|vec| vec.is_empty())
    }

    /// Clears the vector.
    pub fn clear(&self) {
        self.update(|vec| vec.clear());
    }

    /// Extends the vector with elements from an iterator.
    pub fn extend<I>(&self, iter: I)
    where
        I: IntoIterator<Item = T>,
    {
        self.update(|vec| vec.extend(iter));
    }
}

// Option-specific operations
impl<T: Clone> Atomic<Option<T>> {
    /// Returns true if the option is Some.
    pub fn is_some(&self) -> bool {
        self.with(|opt| opt.is_some())
    }

    /// Returns true if the option is None.
    pub fn is_none(&self) -> bool {
        self.with(|opt| opt.is_none())
    }

    /// Takes the value out of the option, leaving None in its place.
    pub fn take(&self) -> Option<T> {
        self.with_mut(|opt| opt.take())
    }

    /// Replaces the value, returning the old value.
    pub fn replace(&self, value: T) -> Option<T> {
        self.with_mut(|opt| opt.replace(value))
    }
}

impl<T: Clone + fmt::Debug> fmt::Debug for Atomic<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = self.load();
        f.debug_struct("Atomic")
            .field("value", &value)
            .finish()
    }
}

impl<T: Default> Default for Atomic<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T> From<T> for Atomic<T> {
    fn from(value: T) -> Self {
        Self::new(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn test_custom_struct() {
        #[derive(Debug, Clone, PartialEq)]
        struct Person {
            name: String,
            age: u32,
        }

        let atomic = Atomic::new(Person {
            name: "Alice".to_string(),
            age: 30,
        });

        let person = atomic.load();
        assert_eq!(person.name, "Alice");
        assert_eq!(person.age, 30);

        atomic.store(Person {
            name: "Bob".to_string(),
            age: 25,
        });

        let person = atomic.load();
        assert_eq!(person.name, "Bob");
    }

    #[test]
    fn test_with_operations() {
        #[derive(Debug, Clone)]
        struct Counter {
            count: u32,
            name: String,
        }

        let atomic = Atomic::new(Counter {
            count: 0,
            name: "test".to_string(),
        });

        // Read with closure
        let count = atomic.with(|c| c.count);
        assert_eq!(count, 0);

        // Modify with closure
        atomic.with_mut(|c| {
            c.count += 10;
            c.name = "updated".to_string();
        });

        let name = atomic.with(|c| c.name.clone());
        assert_eq!(name, "updated");
    }

    #[test]
    fn test_string_operations() {
        let atomic = Atomic::new(String::from("Hello"));

        atomic.push_str(" World");
        assert_eq!(atomic.load(), "Hello World");

        assert_eq!(atomic.len(), 11);
        assert!(!atomic.is_empty());

        atomic.clear();
        assert!(atomic.is_empty());
    }

    #[test]
    fn test_vec_operations() {
        let atomic = Atomic::new(Vec::<i32>::new());

        atomic.push(1);
        atomic.push(2);
        atomic.push(3);

        assert_eq!(atomic.len(), 3);

        let popped = atomic.pop();
        assert_eq!(popped, Some(3));
        assert_eq!(atomic.len(), 2);

        atomic.extend(vec![4, 5, 6]);
        assert_eq!(atomic.len(), 5);

        atomic.clear();
        assert!(atomic.is_empty());
    }

    #[test]
    fn test_option_operations() {
        let atomic = Atomic::new(Some(42));

        assert!(atomic.is_some());
        assert!(!atomic.is_none());

        let value = atomic.take();
        assert_eq!(value, Some(42));
        assert!(atomic.is_none());

        let old = atomic.replace(100);
        assert_eq!(old, None);
        assert_eq!(atomic.load(), Some(100));
    }

    #[test]
    fn test_concurrent_string() {
        let atomic = Arc::new(Atomic::new(String::new()));
        let mut handles = vec![];

        for i in 0..10 {
            let atomic = Arc::clone(&atomic);
            handles.push(thread::spawn(move || {
                atomic.push_str(&format!("{}", i));
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        assert_eq!(atomic.len(), 10);
    }

    #[test]
    fn test_replace_with() {
        #[derive(Debug, Clone, PartialEq)]
        struct Data {
            value: i32,
        }

        let atomic = Atomic::new(Data { value: 10 });

        let old = atomic.replace_with(|d| Data { value: d.value * 2 });
        assert_eq!(old.value, 10);
        assert_eq!(atomic.load().value, 20);
    }

    #[test]
    fn test_try_update() {
        let atomic = Atomic::new(10);

        // Successful update
        let result = atomic.try_update(|x| {
            if *x < 100 {
                *x = *x * 2;
                true
            } else {
                false
            }
        });
        assert!(result.is_ok());
        assert_eq!(atomic.load(), 20);

        // Failed update
        atomic.store(150);
        let result = atomic.try_update(|x| {
            if *x < 100 {
                *x = *x * 2;
                true
            } else {
                false
            }
        });
        assert!(result.is_err());
        assert_eq!(atomic.load(), 150);
    }

    #[test]
    fn test_enum() {
        #[derive(Debug, Clone, PartialEq)]
        enum State {
            Idle,
            Running(u32),
            Stopped,
        }

        let atomic = Atomic::new(State::Idle);

        atomic.store(State::Running(42));
        assert_eq!(atomic.load(), State::Running(42));

        atomic.update(|state| {
            *state = State::Stopped;
        });
        assert_eq!(atomic.load(), State::Stopped);
    }

    #[test]
    fn test_concurrent_complex_type() {
        #[derive(Debug, Clone)]
        struct Task {
            id: u32,
            completed: bool,
            data: Vec<String>,
        }

        let atomic = Arc::new(Atomic::new(Task {
            id: 0,
            completed: false,
            data: Vec::new(),
        }));

        let mut handles = vec![];

        for i in 0..5 {
            let atomic = Arc::clone(&atomic);
            handles.push(thread::spawn(move || {
                atomic.with_mut(|task| {
                    task.id += 1;
                    task.data.push(format!("Thread {}", i));
                });
            }));
        }

        for handle in handles {
            handle.join().unwrap();
        }

        let task = atomic.load();
        assert_eq!(task.id, 5);
        assert_eq!(task.data.len(), 5);
    }
}