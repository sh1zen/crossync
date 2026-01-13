use crate::atomic::AtomicBuffer;
use crate::sync::Backoff;
use crossbeam_utils::CachePadded;
use std::alloc::{Layout, alloc_zeroed, dealloc, handle_alloc_error};
use std::cell::UnsafeCell;
use std::fmt;
use std::iter::FromIterator;
use std::mem::{self, MaybeUninit};
use std::ptr;
use std::sync::atomic::{AtomicPtr, AtomicU64, AtomicUsize, Ordering, fence};

const BLOCK_CAP: usize = 32;
const BLOCK_CAP_MASK: usize = BLOCK_CAP - 1;
const BLOCK_SHIFT: u32 = BLOCK_CAP.trailing_zeros();

const INDEX_SHIFT: usize = 1;
const HAS_NEXT: usize = 1;

const WRITE: usize = 1;
const READ: usize = 2;

const PENDING_SHIFT: u32 = 32;
const READ_MASK: u64 = 0xFFFF_FFFF;
const PENDING_ONE: u64 = 1 << PENDING_SHIFT;

#[inline(always)]
fn pack_counters(pending: u32, read: u32) -> u64 {
    ((pending as u64) << PENDING_SHIFT) | (read as u64)
}

#[inline(always)]
fn unpack_pending(val: u64) -> u32 {
    (val >> PENDING_SHIFT) as u32
}

#[inline(always)]
fn unpack_read(val: u64) -> u32 {
    (val & READ_MASK) as u32
}

#[repr(C)]
struct Slot<T> {
    state: AtomicUsize,
    value: UnsafeCell<MaybeUninit<T>>,
}

impl<T> Slot<T> {
    #[inline(always)]
    unsafe fn wait_write_raw(state: *const AtomicUsize) {
        let backoff = Backoff::new();
        unsafe {
            while (*state).load(Ordering::Acquire) & WRITE == 0 {
                backoff.snooze();
            }
        }
    }
}

#[repr(C)]
struct Block<T> {
    next: AtomicPtr<Block<T>>,
    counters: CachePadded<AtomicU64>,
    slots: [Slot<T>; BLOCK_CAP],
}

impl<T> Block<T> {
    const LAYOUT: Layout = Layout::new::<Self>();

    #[inline]
    fn new() -> *mut Self {
        let ptr = unsafe { alloc_zeroed(Self::LAYOUT) };
        if ptr.is_null() {
            handle_alloc_error(Self::LAYOUT)
        }
        ptr.cast()
    }

    #[inline]
    unsafe fn reset(ptr: *mut Self) {
        unsafe {
            (*ptr).next.store(ptr::null_mut(), Ordering::Relaxed);
            (*ptr).counters.store(0, Ordering::Relaxed);
            let slots = &(*ptr).slots;
            for slot in slots.iter() {
                slot.state.store(0, Ordering::Relaxed);
            }
        }
    }

    #[inline(always)]
    fn inc_pending(&self) -> u64 {
        self.counters.fetch_add(PENDING_ONE, Ordering::Acquire)
    }

    #[inline(always)]
    fn dec_pending_inc_read(&self) -> (u32, u32) {
        let delta = 1u64.wrapping_sub(PENDING_ONE);
        let old = self.counters.fetch_add(delta, Ordering::AcqRel);
        let new = old.wrapping_add(delta);
        (unpack_pending(new), unpack_read(new))
    }

    #[inline(always)]
    fn dec_pending(&self) {
        self.counters.fetch_sub(PENDING_ONE, Ordering::Release);
    }

    #[inline(always)]
    fn wait_next(&self) -> *mut Self {
        let mut next = self.next.load(Ordering::Acquire);
        if !next.is_null() {
            return next;
        }
        let backoff = Backoff::new();
        loop {
            backoff.snooze();
            next = self.next.load(Ordering::Acquire);
            if !next.is_null() {
                return next;
            }
        }
    }

    #[inline(always)]
    fn get_next(&self) -> *mut Self {
        self.next.load(Ordering::Acquire)
    }

    #[inline]
    unsafe fn dealloc(ptr: *mut Self) {
        unsafe { dealloc(ptr.cast(), Self::LAYOUT) };
    }
}

struct BlockArray<T> {
    entries: [CachePadded<BlockEntry<T>>; BLOCK_CAP],
}

struct BlockEntry<T> {
    tagged: AtomicUsize,
    ptr: AtomicPtr<Block<T>>,
}

impl<T> BlockArray<T> {
    fn new() -> Self {
        const INIT: CachePadded<BlockEntry<()>> = CachePadded::new(BlockEntry {
            tagged: AtomicUsize::new(usize::MAX),
            ptr: AtomicPtr::new(ptr::null_mut()),
        });
        unsafe {
            Self {
                entries: mem::transmute([INIT; BLOCK_CAP]),
            }
        }
    }

    #[inline(always)]
    fn get(&self, idx: usize) -> *mut Block<T> {
        let e = unsafe { self.entries.get_unchecked(idx & BLOCK_CAP_MASK) };
        if e.tagged.load(Ordering::Acquire) == idx {
            e.ptr.load(Ordering::Relaxed)
        } else {
            ptr::null_mut()
        }
    }

    #[inline(always)]
    fn set(&self, idx: usize, block: *mut Block<T>) {
        let e = unsafe { self.entries.get_unchecked(idx & BLOCK_CAP_MASK) };
        e.ptr.store(block, Ordering::Relaxed);
        e.tagged.store(idx, Ordering::Release);
    }

    #[inline(always)]
    fn clear(&self, idx: usize) {
        let e = unsafe { self.entries.get_unchecked(idx & BLOCK_CAP_MASK) };
        e.tagged.store(usize::MAX, Ordering::Release);
    }
}

#[repr(C, align(128))]
struct Position<T> {
    index: AtomicUsize,
    block: AtomicPtr<Block<T>>,
}

struct RecycleEntry<T> {
    block: *mut Block<T>,
    block_idx: usize,
}

struct InnerVec<T> {
    head: CachePadded<Position<T>>,
    tail: CachePadded<Position<T>>,
    len: CachePadded<AtomicUsize>,
    block_array: BlockArray<T>,
    free_list: AtomicBuffer<Block<T>>,
    recycle_queue: AtomicBuffer<RecycleEntry<T>>,
    ref_count: AtomicUsize,
    exclusive_lock: AtomicUsize,
}

#[repr(transparent)]
pub struct AtomicVec<T> {
    inner: *const InnerVec<T>,
}

unsafe impl<T: Send> Send for AtomicVec<T> {}
unsafe impl<T: Send> Sync for AtomicVec<T> {}

impl<T> AtomicVec<T> {
    #[inline]
    pub fn new() -> Self {
        let inner = InnerVec {
            head: CachePadded::new(Position {
                block: AtomicPtr::new(ptr::null_mut()),
                index: AtomicUsize::new(0),
            }),
            tail: CachePadded::new(Position {
                block: AtomicPtr::new(ptr::null_mut()),
                index: AtomicUsize::new(0),
            }),
            len: CachePadded::new(AtomicUsize::new(0)),
            block_array: BlockArray::new(),
            free_list: AtomicBuffer::with_capacity(64),
            recycle_queue: AtomicBuffer::with_capacity(32),
            ref_count: AtomicUsize::new(1),
            exclusive_lock: AtomicUsize::new(0),
        };

        let block = Block::<T>::new();
        inner.tail.block.store(block, Ordering::Relaxed);
        inner.head.block.store(block, Ordering::Relaxed);
        inner.block_array.set(0, block);

        Self {
            inner: Box::into_raw(Box::new(inner)),
        }
    }

    #[inline]
    pub fn with_capacity(cap: usize) -> Self {
        let vec = Self::new();
        let blocks_needed = (cap + BLOCK_CAP - 1) / BLOCK_CAP;
        for _ in 1..blocks_needed {
            let block = Block::<T>::new();
            if vec.inner().free_list.push(block).is_err() {
                unsafe { Block::dealloc(block) };
            }
        }
        vec
    }

    pub fn init_with<F: FnMut() -> T>(cap: usize, mut init: F) -> Self {
        let vec = Self::with_capacity(cap);
        for _ in 0..cap {
            vec.push(init());
        }
        vec
    }

    #[inline(always)]
    fn inner(&self) -> &InnerVec<T> {
        unsafe { &*self.inner }
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        self.inner().len.load(Ordering::Relaxed)
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline]
    pub fn capacity(&self) -> usize {
        let inner = self.inner();
        let head = inner.head.index.load(Ordering::Relaxed) >> INDEX_SHIFT;
        let tail = inner.tail.index.load(Ordering::Relaxed) >> INDEX_SHIFT;
        ((tail.wrapping_sub(head)) & !BLOCK_CAP_MASK) + BLOCK_CAP
    }

    #[inline]
    unsafe fn try_process_recycles(&self) {
        let inner = self.inner();
        for _ in 0..2 {
            if let Some(entry_ptr) = inner.recycle_queue.pop() {
                let entry = unsafe { Box::from_raw(entry_ptr) };
                let counters = unsafe { (*entry.block).counters.load(Ordering::Acquire) };
                if unpack_pending(counters) == 0 {
                    inner.block_array.clear(entry.block_idx);
                    unsafe {
                        Block::reset(entry.block);
                    }
                    // FIX: Deallocate if free_list is full
                    if inner.free_list.push(entry.block).is_err() {
                        unsafe {
                            Block::dealloc(entry.block);
                        }
                    }
                } else {
                    let _ = inner.recycle_queue.push(Box::into_raw(entry));
                    break;
                }
            } else {
                break;
            }
        }
    }

    #[inline]
    unsafe fn acquire_block(&self) -> *mut Block<T> {
        let inner = self.inner();
        inner.free_list.pop().unwrap_or_else(Block::<T>::new)
    }

    #[inline]
    fn wait_exclusive(&self) {
        let inner = self.inner();
        let backoff = Backoff::new();
        while inner
            .exclusive_lock
            .compare_exchange_weak(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            backoff.snooze();
        }
    }

    #[inline]
    fn release_exclusive(&self) {
        self.inner().exclusive_lock.store(0, Ordering::Release);
    }

    #[inline]
    pub fn push(&self, value: T) {
        unsafe {
            let inner = self.inner();
            self.try_process_recycles();

            let backoff = Backoff::new();
            let mut tail = inner.tail.index.load(Ordering::Acquire);
            let mut block = inner.tail.block.load(Ordering::Acquire);

            loop {
                let offset = (tail >> INDEX_SHIFT) & BLOCK_CAP_MASK;

                if offset == BLOCK_CAP - 1 {
                    backoff.snooze();
                    tail = inner.tail.index.load(Ordering::Acquire);
                    block = inner.tail.block.load(Ordering::Acquire);
                    continue;
                }

                let new_tail = tail + (1 << INDEX_SHIFT);

                match inner.tail.index.compare_exchange_weak(
                    tail,
                    new_tail,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => {
                        if offset + 1 == BLOCK_CAP - 1 {
                            let next = self.acquire_block();
                            let next_index = new_tail.wrapping_add(1 << INDEX_SHIFT);
                            let block_idx = (new_tail >> INDEX_SHIFT) >> BLOCK_SHIFT;

                            (*block).next.store(next, Ordering::Release);
                            inner.block_array.set(block_idx + 1, next);
                            inner.tail.block.store(next, Ordering::Release);
                            inner.tail.index.store(next_index, Ordering::Release);
                        }

                        let slot = (*block).slots.get_unchecked(offset);
                        slot.value.get().write(MaybeUninit::new(value));
                        slot.state.store(WRITE, Ordering::Release);

                        inner.len.fetch_add(1, Ordering::Relaxed);
                        return;
                    }
                    Err(t) => {
                        tail = t;
                        block = inner.tail.block.load(Ordering::Acquire);
                        backoff.spin();
                    }
                }
            }
        }
    }

    #[inline]
    pub fn push_batch<I: IntoIterator<Item = T>>(&self, iter: I) {
        for value in iter {
            self.push(value);
        }
    }

    #[inline]
    pub fn pop(&self) -> Option<T> {
        unsafe {
            let inner = self.inner();

            if inner.len.load(Ordering::Relaxed) == 0 {
                return None;
            }

            let backoff = Backoff::new();
            let mut head = inner.head.index.load(Ordering::Acquire);
            let mut block = inner.head.block.load(Ordering::Acquire);

            loop {
                let offset = (head >> INDEX_SHIFT) & BLOCK_CAP_MASK;

                if offset == BLOCK_CAP - 1 || block.is_null() {
                    backoff.snooze();
                    head = inner.head.index.load(Ordering::Acquire);
                    block = inner.head.block.load(Ordering::Acquire);
                    continue;
                }

                let mut new_head = head + (1 << INDEX_SHIFT);

                if new_head & HAS_NEXT == 0 {
                    fence(Ordering::SeqCst);
                    let tail = inner.tail.index.load(Ordering::Relaxed);
                    let head_idx = head >> INDEX_SHIFT;
                    let tail_idx = tail >> INDEX_SHIFT;

                    if head_idx == tail_idx {
                        return None;
                    }

                    if (head_idx >> BLOCK_SHIFT) != (tail_idx >> BLOCK_SHIFT) {
                        new_head |= HAS_NEXT;
                    }
                }

                (*block).inc_pending();

                match inner.head.index.compare_exchange_weak(
                    head,
                    new_head,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => {
                        if offset + 1 == BLOCK_CAP - 1 {
                            let next = (*block).wait_next();
                            let mut next_index =
                                (new_head & !HAS_NEXT).wrapping_add(1 << INDEX_SHIFT);

                            if !(*next).get_next().is_null() {
                                next_index |= HAS_NEXT;
                            }

                            inner.head.block.store(next, Ordering::Release);
                            inner.head.index.store(next_index, Ordering::Release);
                        }

                        let slot = (*block).slots.get_unchecked(offset);
                        Slot::<T>::wait_write_raw(&slot.state as *const _);
                        let value = slot.value.get().read().assume_init();
                        slot.state.store(READ, Ordering::Relaxed);

                        inner.len.fetch_sub(1, Ordering::Relaxed);

                        let (pending, read_count) = (*block).dec_pending_inc_read();

                        if read_count == (BLOCK_CAP - 1) as u32 {
                            let block_idx = (head >> INDEX_SHIFT) >> BLOCK_SHIFT;
                            if pending == 0 {
                                inner.block_array.clear(block_idx);
                                Block::reset(block);
                                // FIX: Deallocate if free_list is full
                                if inner.free_list.push(block).is_err() {
                                    Block::dealloc(block);
                                }
                            } else {
                                let entry =
                                    Box::into_raw(Box::new(RecycleEntry { block, block_idx }));
                                if inner.recycle_queue.push(entry).is_err() {
                                    // FIX: Deallocate both entry and block
                                    drop(Box::from_raw(entry));
                                    Block::dealloc(block);
                                }
                            }
                        }

                        return Some(value);
                    }
                    Err(h) => {
                        (*block).dec_pending();
                        head = h;
                        block = inner.head.block.load(Ordering::Acquire);
                        backoff.spin();
                    }
                }
            }
        }
    }

    #[inline]
    pub fn pop_batch(&self, count: usize) -> Vec<T> {
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            match self.pop() {
                Some(v) => out.push(v),
                None => break,
            }
        }
        out
    }

    pub fn drain(&self) -> Vec<T> {
        let len = self.len();
        self.pop_batch(len)
    }

    pub fn reset_with(&self, cap: usize, mut init: impl FnMut() -> T) -> usize {
        self.wait_exclusive();

        let inner = self.inner();

        while self.pop().is_some() {}

        unsafe {
            while let Some(entry_ptr) = inner.recycle_queue.pop() {
                let entry = Box::from_raw(entry_ptr);
                inner.block_array.clear(entry.block_idx);
                Block::reset(entry.block);
                if inner.free_list.push(entry.block).is_err() {
                    Block::dealloc(entry.block);
                }
            }
        }

        let old_block = inner.head.block.load(Ordering::Relaxed);

        inner.head.index.store(0, Ordering::Relaxed);
        inner.tail.index.store(0, Ordering::Relaxed);
        inner.len.store(0, Ordering::Relaxed);

        for i in 0..BLOCK_CAP {
            inner.block_array.set(i, ptr::null_mut());
        }

        unsafe {
            if !old_block.is_null() {
                Block::reset(old_block);
                inner.tail.block.store(old_block, Ordering::Relaxed);
                inner.head.block.store(old_block, Ordering::Relaxed);
                inner.block_array.set(0, old_block);
            } else {
                let block = self.acquire_block();
                inner.tail.block.store(block, Ordering::Relaxed);
                inner.head.block.store(block, Ordering::Relaxed);
                inner.block_array.set(0, block);
            }
        }

        for _ in 0..cap {
            self.push(init());
        }

        self.release_exclusive();
        cap
    }

    pub fn as_vec(&self) -> Vec<T> {
        self.wait_exclusive();
        let out = self.drain();
        self.release_exclusive();
        out
    }
}

impl<T: PartialEq> AtomicVec<T> {
    pub fn index_of(&self, value: &T) -> Option<usize> {
        self.wait_exclusive();
        let result = self.index_of_inner(value);
        self.release_exclusive();
        result
    }

    fn index_of_inner(&self, value: &T) -> Option<usize> {
        unsafe {
            let inner = self.inner();
            let mut block = inner.head.block.load(Ordering::Acquire);
            let head = inner.head.index.load(Ordering::Acquire);
            let tail = inner.tail.index.load(Ordering::Acquire);

            let start_idx = head >> INDEX_SHIFT;
            let end_idx = tail >> INDEX_SHIFT;
            let mut logical_idx = 0usize;

            let mut block_start = start_idx & !BLOCK_CAP_MASK;
            let mut offset = start_idx & BLOCK_CAP_MASK;

            while block_start < end_idx && !block.is_null() {
                let block_end = ((block_start + BLOCK_CAP) - 1).min(end_idx);

                while (block_start + offset) < block_end && offset < BLOCK_CAP - 1 {
                    let slot = &(*block).slots[offset];
                    if slot.state.load(Ordering::Acquire) & WRITE != 0 {
                        let v = &*(*slot.value.get()).as_ptr();
                        if v == value {
                            return Some(logical_idx);
                        }
                    }
                    offset += 1;
                    logical_idx += 1;
                }

                block = (*block).next.load(Ordering::Acquire);
                block_start += BLOCK_CAP;
                offset = 0;
            }
            None
        }
    }

    #[inline]
    pub fn contains(&self, value: &T) -> bool {
        self.index_of(value).is_some()
    }
}

impl<T> AtomicVec<T> {
    pub fn find<F>(&self, predicate: F) -> Option<T>
    where
        F: Fn(&T) -> bool,
        T: Clone,
    {
        self.wait_exclusive();
        let result = self.find_inner(predicate);
        self.release_exclusive();
        result
    }

    fn find_inner<F>(&self, predicate: F) -> Option<T>
    where
        F: Fn(&T) -> bool,
        T: Clone,
    {
        unsafe {
            let inner = self.inner();
            let mut block = inner.head.block.load(Ordering::Acquire);
            let head = inner.head.index.load(Ordering::Acquire);
            let tail = inner.tail.index.load(Ordering::Acquire);

            let start_idx = head >> INDEX_SHIFT;
            let end_idx = tail >> INDEX_SHIFT;

            let mut block_start = start_idx & !BLOCK_CAP_MASK;
            let mut offset = start_idx & BLOCK_CAP_MASK;

            while block_start < end_idx && !block.is_null() {
                let block_end = ((block_start + BLOCK_CAP) - 1).min(end_idx);

                while (block_start + offset) < block_end && offset < BLOCK_CAP - 1 {
                    let slot = &(*block).slots[offset];
                    if slot.state.load(Ordering::Acquire) & WRITE != 0 {
                        let v = &*(*slot.value.get()).as_ptr();
                        if predicate(v) {
                            return Some(v.clone());
                        }
                    }
                    offset += 1;
                }

                block = (*block).next.load(Ordering::Acquire);
                block_start += BLOCK_CAP;
                offset = 0;
            }
            None
        }
    }

    pub fn for_each<F>(&self, f: F)
    where
        F: Fn(&T),
    {
        self.wait_exclusive();
        self.for_each_inner(f);
        self.release_exclusive();
    }

    fn for_each_inner<F>(&self, f: F)
    where
        F: Fn(&T),
    {
        unsafe {
            let inner = self.inner();
            let mut block = inner.head.block.load(Ordering::Acquire);
            let head = inner.head.index.load(Ordering::Acquire);
            let tail = inner.tail.index.load(Ordering::Acquire);

            let start_idx = head >> INDEX_SHIFT;
            let end_idx = tail >> INDEX_SHIFT;

            let mut block_start = start_idx & !BLOCK_CAP_MASK;
            let mut offset = start_idx & BLOCK_CAP_MASK;

            while block_start < end_idx && !block.is_null() {
                let block_end = ((block_start + BLOCK_CAP) - 1).min(end_idx);

                while (block_start + offset) < block_end && offset < BLOCK_CAP - 1 {
                    let slot = &(*block).slots[offset];
                    if slot.state.load(Ordering::Acquire) & WRITE != 0 {
                        let v = &*(*slot.value.get()).as_ptr();
                        f(v);
                    }
                    offset += 1;
                }

                block = (*block).next.load(Ordering::Acquire);
                block_start += BLOCK_CAP;
                offset = 0;
            }
        }
    }

    pub fn fold<B, F>(&self, init: B, f: F) -> B
    where
        F: Fn(B, &T) -> B,
    {
        self.wait_exclusive();
        let result = self.fold_inner(init, f);
        self.release_exclusive();
        result
    }

    fn fold_inner<B, F>(&self, init: B, f: F) -> B
    where
        F: Fn(B, &T) -> B,
    {
        unsafe {
            let inner = self.inner();
            let mut block = inner.head.block.load(Ordering::Acquire);
            let head = inner.head.index.load(Ordering::Acquire);
            let tail = inner.tail.index.load(Ordering::Acquire);

            let start_idx = head >> INDEX_SHIFT;
            let end_idx = tail >> INDEX_SHIFT;

            let mut acc = init;
            let mut block_start = start_idx & !BLOCK_CAP_MASK;
            let mut offset = start_idx & BLOCK_CAP_MASK;

            while block_start < end_idx && !block.is_null() {
                let block_end = ((block_start + BLOCK_CAP) - 1).min(end_idx);

                while (block_start + offset) < block_end && offset < BLOCK_CAP - 1 {
                    let slot = &(*block).slots[offset];
                    if slot.state.load(Ordering::Acquire) & WRITE != 0 {
                        let v = &*(*slot.value.get()).as_ptr();
                        acc = f(acc, v);
                    }
                    offset += 1;
                }

                block = (*block).next.load(Ordering::Acquire);
                block_start += BLOCK_CAP;
                offset = 0;
            }
            acc
        }
    }

    pub fn reduce<F>(&self, f: F) -> Option<T>
    where
        F: Fn(T, &T) -> T,
        T: Clone,
    {
        self.wait_exclusive();
        let result = self.reduce_inner(f);
        self.release_exclusive();
        result
    }

    fn reduce_inner<F>(&self, f: F) -> Option<T>
    where
        F: Fn(T, &T) -> T,
        T: Clone,
    {
        unsafe {
            let inner = self.inner();
            let mut block = inner.head.block.load(Ordering::Acquire);
            let head = inner.head.index.load(Ordering::Acquire);
            let tail = inner.tail.index.load(Ordering::Acquire);

            let start_idx = head >> INDEX_SHIFT;
            let end_idx = tail >> INDEX_SHIFT;

            if start_idx >= end_idx {
                return None;
            }

            let mut acc: Option<T> = None;
            let mut block_start = start_idx & !BLOCK_CAP_MASK;
            let mut offset = start_idx & BLOCK_CAP_MASK;

            while block_start < end_idx && !block.is_null() {
                let block_end = ((block_start + BLOCK_CAP) - 1).min(end_idx);

                while (block_start + offset) < block_end && offset < BLOCK_CAP - 1 {
                    let slot = &(*block).slots[offset];
                    if slot.state.load(Ordering::Acquire) & WRITE != 0 {
                        let v = &*(*slot.value.get()).as_ptr();
                        acc = Some(match acc {
                            None => v.clone(),
                            Some(a) => f(a, v),
                        });
                    }
                    offset += 1;
                }

                block = (*block).next.load(Ordering::Acquire);
                block_start += BLOCK_CAP;
                offset = 0;
            }
            acc
        }
    }

    pub fn get(&self, index: usize) -> Option<T>
    where
        T: Clone,
    {
        self.wait_exclusive();
        let result = self.get_inner(index);
        self.release_exclusive();
        result
    }

    fn get_inner(&self, index: usize) -> Option<T>
    where
        T: Clone,
    {
        unsafe {
            let inner = self.inner();
            let head = inner.head.index.load(Ordering::Acquire);
            let tail = inner.tail.index.load(Ordering::Acquire);

            let start_idx = head >> INDEX_SHIFT;
            let end_idx = tail >> INDEX_SHIFT;
            let len = self.calc_len(start_idx, end_idx);

            if index >= len {
                return None;
            }

            let target_abs = start_idx + index;
            let offset = target_abs & BLOCK_CAP_MASK;

            let adjusted_offset = offset + (target_abs / (BLOCK_CAP - 1));
            let adjusted_block_idx = (start_idx + adjusted_offset) >> BLOCK_SHIFT;
            let final_offset = (start_idx + adjusted_offset) & BLOCK_CAP_MASK;

            let block = self.get_block_at(adjusted_block_idx, start_idx >> BLOCK_SHIFT);
            if block.is_null() || final_offset >= BLOCK_CAP - 1 {
                return None;
            }

            let slot = &(*block).slots[final_offset];
            if slot.state.load(Ordering::Acquire) & WRITE != 0 {
                Some((*(*slot.value.get()).as_ptr()).clone())
            } else {
                None
            }
        }
    }

    fn calc_len(&self, start: usize, end: usize) -> usize {
        if end <= start {
            return 0;
        }
        let total_slots = end - start;
        let full_blocks = total_slots / BLOCK_CAP;
        let remainder = total_slots % BLOCK_CAP;
        full_blocks * (BLOCK_CAP - 1) + remainder.min(BLOCK_CAP - 1)
    }

    unsafe fn get_block_at(&self, target_block: usize, start_block: usize) -> *mut Block<T> {
        let inner = self.inner();

        let cached = inner.block_array.get(target_block);
        if !cached.is_null() {
            return cached;
        }

        let mut block = inner.head.block.load(Ordering::Acquire);
        let mut current_block = start_block;

        while current_block < target_block && !block.is_null() {
            block = unsafe { (*block).next.load(Ordering::Acquire) };
            current_block += 1;
        }
        block
    }

    pub fn swap(&self, i: usize, j: usize) -> bool {
        if i == j {
            return true;
        }
        self.wait_exclusive();
        let result = self.swap_inner(i, j);
        self.release_exclusive();
        result
    }

    fn swap_inner(&self, i: usize, j: usize) -> bool {
        unsafe {
            let inner = self.inner();
            let head = inner.head.index.load(Ordering::Acquire);
            let tail = inner.tail.index.load(Ordering::Acquire);
            let start_idx = head >> INDEX_SHIFT;
            let end_idx = tail >> INDEX_SHIFT;
            let len = self.calc_len(start_idx, end_idx);

            if i >= len || j >= len {
                return false;
            }

            let (ptr_i, ptr_j) = (
                self.get_slot_ptr(i, start_idx),
                self.get_slot_ptr(j, start_idx),
            );

            if ptr_i.is_null() || ptr_j.is_null() {
                return false;
            }

            ptr::swap((*ptr_i).value.get(), (*ptr_j).value.get());
            true
        }
    }

    unsafe fn get_slot_ptr(&self, index: usize, start_idx: usize) -> *mut Slot<T> {
        let slots_per_block = BLOCK_CAP - 1;
        let block_num = index / slots_per_block;
        let slot_in_block = index % slots_per_block;

        let target_block = (start_idx >> BLOCK_SHIFT) + block_num;
        let block = unsafe { self.get_block_at(target_block, start_idx >> BLOCK_SHIFT) };

        if block.is_null() {
            return ptr::null_mut();
        }

        let base_offset = if block_num == 0 {
            start_idx & BLOCK_CAP_MASK
        } else {
            0
        };

        let offset = base_offset + slot_in_block;
        if offset >= BLOCK_CAP - 1 {
            return ptr::null_mut();
        }

        unsafe { (*block).slots.as_mut_ptr().add(offset) }
    }

    pub fn remove(&self, index: usize) -> Option<T> {
        self.wait_exclusive();
        let result = self.remove_inner(index);
        self.release_exclusive();
        result
    }

    fn remove_inner(&self, index: usize) -> Option<T> {
        let len = self.len();
        if index >= len {
            return None;
        }

        let mut elements = Vec::with_capacity(len);
        while let Some(v) = self.pop_internal() {
            elements.push(v);
        }

        if index >= elements.len() {
            for v in elements {
                self.push(v);
            }
            return None;
        }

        let removed = elements.remove(index);

        for v in elements {
            self.push(v);
        }

        Some(removed)
    }

    pub fn reverse(&self) {
        self.wait_exclusive();
        self.reverse_inner();
        self.release_exclusive();
    }

    fn reverse_inner(&self) {
        let mut elements = Vec::new();
        while let Some(v) = self.pop_internal() {
            elements.push(v);
        }

        for v in elements.into_iter().rev() {
            self.push(v);
        }
    }

    fn pop_internal(&self) -> Option<T> {
        unsafe {
            let inner = self.inner();

            if inner.len.load(Ordering::Relaxed) == 0 {
                return None;
            }

            let backoff = Backoff::new();
            let mut head = inner.head.index.load(Ordering::Acquire);
            let mut block = inner.head.block.load(Ordering::Acquire);

            loop {
                let offset = (head >> INDEX_SHIFT) & BLOCK_CAP_MASK;

                if offset == BLOCK_CAP - 1 || block.is_null() {
                    backoff.snooze();
                    head = inner.head.index.load(Ordering::Acquire);
                    block = inner.head.block.load(Ordering::Acquire);
                    continue;
                }

                let mut new_head = head + (1 << INDEX_SHIFT);

                if new_head & HAS_NEXT == 0 {
                    fence(Ordering::SeqCst);
                    let tail = inner.tail.index.load(Ordering::Relaxed);
                    let head_idx = head >> INDEX_SHIFT;
                    let tail_idx = tail >> INDEX_SHIFT;

                    if head_idx == tail_idx {
                        return None;
                    }

                    if (head_idx >> BLOCK_SHIFT) != (tail_idx >> BLOCK_SHIFT) {
                        new_head |= HAS_NEXT;
                    }
                }

                (*block).inc_pending();

                match inner.head.index.compare_exchange_weak(
                    head,
                    new_head,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => {
                        if offset + 1 == BLOCK_CAP - 1 {
                            let next = (*block).wait_next();
                            let mut next_index =
                                (new_head & !HAS_NEXT).wrapping_add(1 << INDEX_SHIFT);

                            if !(*next).get_next().is_null() {
                                next_index |= HAS_NEXT;
                            }

                            inner.head.block.store(next, Ordering::Release);
                            inner.head.index.store(next_index, Ordering::Release);
                        }

                        let slot = (*block).slots.get_unchecked(offset);
                        Slot::<T>::wait_write_raw(&slot.state as *const _);
                        let value = slot.value.get().read().assume_init();
                        slot.state.store(READ, Ordering::Relaxed);

                        inner.len.fetch_sub(1, Ordering::Relaxed);

                        let (pending, read_count) = (*block).dec_pending_inc_read();

                        if read_count == (BLOCK_CAP - 1) as u32 {
                            let block_idx = (head >> INDEX_SHIFT) >> BLOCK_SHIFT;
                            if pending == 0 {
                                inner.block_array.clear(block_idx);
                                Block::reset(block);
                                // FIX: Deallocate if free_list is full
                                if inner.free_list.push(block).is_err() {
                                    Block::dealloc(block);
                                }
                            } else {
                                let entry =
                                    Box::into_raw(Box::new(RecycleEntry { block, block_idx }));
                                if inner.recycle_queue.push(entry).is_err() {
                                    // FIX: Deallocate both entry and block
                                    drop(Box::from_raw(entry));
                                    Block::dealloc(block);
                                }
                            }
                        }

                        return Some(value);
                    }
                    Err(h) => {
                        (*block).dec_pending();
                        head = h;
                        block = inner.head.block.load(Ordering::Acquire);
                        backoff.spin();
                    }
                }
            }
        }
    }
}

impl<T> Clone for AtomicVec<T> {
    fn clone(&self) -> Self {
        self.inner().ref_count.fetch_add(1, Ordering::Relaxed);
        Self { inner: self.inner }
    }
}

impl<T> Drop for AtomicVec<T> {
    fn drop(&mut self) {
        let inner = unsafe { &*self.inner };

        if inner.ref_count.fetch_sub(1, Ordering::Release) != 1 {
            return;
        }
        fence(Ordering::Acquire);

        unsafe {
            let mut block = inner.head.block.load(Ordering::Relaxed);
            while !block.is_null() {
                let next = (*block).next.load(Ordering::Relaxed);

                if mem::needs_drop::<T>() {
                    for slot in &(*block).slots {
                        let state = slot.state.load(Ordering::Relaxed);
                        if state & WRITE != 0 && state & READ == 0 {
                            ptr::drop_in_place((*slot.value.get()).as_mut_ptr());
                        }
                    }
                }

                Block::dealloc(block);
                block = next;
            }

            for b in inner.free_list.drain_all() {
                Block::dealloc(b);
            }

            for entry_ptr in inner.recycle_queue.drain_all() {
                let entry = Box::from_raw(entry_ptr);
                Block::dealloc(entry.block);
            }

            drop(Box::from_raw(self.inner as *mut InnerVec<T>));
        }
    }
}

impl<T> FromIterator<T> for AtomicVec<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let vec = Self::new();
        vec.push_batch(iter);
        vec
    }
}

impl<T> Default for AtomicVec<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: fmt::Debug> fmt::Debug for AtomicVec<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AtomicVec")
            .field("len", &self.len())
            .field("capacity", &self.capacity())
            .finish()
    }
}

unsafe impl<T: Send> Send for RecycleEntry<T> {}
unsafe impl<T: Send> Sync for RecycleEntry<T> {}
