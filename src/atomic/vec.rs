use crate::atomic::AtomicBuffer;
use crate::core::coord::{Coord, Exclusive};
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

impl<T> Drop for Slot<T> {
    fn drop(&mut self) {
        let state = self.state.get_mut();
        if *state & WRITE != 0 && *state & READ == 0 {
            *state = READ;
            unsafe {
                self.value.get_mut().assume_init_drop();
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
        unsafe {
            drop(Box::from_raw(ptr));
        };
    }

    /// No slot owns a value: only use for fresh, pooled or fully read blocks.
    #[inline]
    unsafe fn dealloc_empty(ptr: *mut Self) {
        unsafe {
            dealloc(ptr.cast(), Self::LAYOUT);
        }
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

struct InnerVec<T> {
    head: CachePadded<Position<T>>,
    tail: CachePadded<Position<T>>,
    block_array: BlockArray<T>,
    free_list: AtomicBuffer<Block<T>>,
    retired: AtomicPtr<Block<T>>,
    retired_count: AtomicUsize,
    ref_count: AtomicUsize,
    coord_lock: Coord,
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
            block_array: BlockArray::new(),
            free_list: AtomicBuffer::with_capacity(512),
            retired: AtomicPtr::new(ptr::null_mut()),
            retired_count: AtomicUsize::new(0),
            ref_count: AtomicUsize::new(1),
            coord_lock: Coord::new(),
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
        let blocks_needed = cap.div_ceil(BLOCK_CAP).min(65);
        for _ in 1..blocks_needed {
            let block = Block::<T>::new();
            if unsafe { vec.inner().free_list.push(block) }.is_err() {
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
        let inner = self.inner();
        let head = inner.head.index.load(Ordering::Acquire) >> INDEX_SHIFT;
        let tail = inner.tail.index.load(Ordering::Acquire) >> INDEX_SHIFT;
        self.calc_len(head, tail)
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

    /// Called only under exclusive coordination, after all pointer users leave.
    unsafe fn reclaim_retired(&self) {
        let inner = self.inner();
        let mut block = inner.retired.swap(ptr::null_mut(), Ordering::Relaxed);
        inner.retired_count.store(0, Ordering::Relaxed);
        unsafe {
            while !block.is_null() {
                let next = (*block).next.load(Ordering::Relaxed);
                // No shared operations run during this grace period, so the
                // pool length is exact. Fully read blocks need no slot reset
                // or value drop when the bounded pool is already full.
                if inner.free_list.len_approx() == inner.free_list.capacity() {
                    Block::dealloc_empty(block);
                } else {
                    Block::reset(block);
                    if inner.free_list.push(block).is_err() {
                        Block::dealloc_empty(block);
                    }
                }
                block = next;
            }
        }
    }

    /// Retirement cannot recycle memory while any shared operation is active.
    unsafe fn retire(&self, block: *mut Block<T>) -> usize {
        let inner = self.inner();
        let mut next = inner.retired.load(Ordering::Relaxed);
        loop {
            unsafe {
                (*block).next.store(next, Ordering::Relaxed);
            }
            match inner.retired.compare_exchange_weak(
                next,
                block,
                Ordering::Release,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(current) => next = current,
            }
        }
        inner.retired_count.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn try_reclaim(&self) {
        let inner = self.inner();
        let count = inner.retired_count.load(Ordering::Relaxed);
        if count == 0 {
            return;
        }
        // Force a grace period at a bounded backlog, even under continuous load.
        let guard = if count >= 512 {
            Some(inner.coord_lock.exclusive())
        } else {
            inner.coord_lock.try_exclusive()
        };
        if let Some(_guard) = guard {
            unsafe {
                self.reclaim_retired();
            }
        }
    }

    #[inline]
    unsafe fn acquire_block(&self) -> *mut Block<T> {
        let pool = &self.inner().free_list;
        if pool.is_empty_fast() {
            Block::<T>::new()
        } else {
            pool.pop().unwrap_or_else(Block::<T>::new)
        }
    }

    #[inline]
    fn wait_exclusive(&self) -> Exclusive<'_> {
        self.inner().coord_lock.exclusive()
    }

    #[inline]
    pub fn push(&self, value: T) {
        let _coord = self.inner().coord_lock.shared();
        self.push_internal(value);
    }

    #[inline(always)]
    fn push_internal(&self, value: T) {
        unsafe {
            let inner = self.inner();

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
        let coord = self.inner().coord_lock.shared();
        let (result, reclaim) = self.pop_raw();
        drop(coord);
        if reclaim {
            self.try_reclaim();
        }
        result
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
        let _coord = self.wait_exclusive();

        let inner = self.inner();

        while self.pop_internal().is_some() {}

        unsafe {
            self.reclaim_retired();
        }

        let old_block = inner.head.block.load(Ordering::Relaxed);

        inner.head.index.store(0, Ordering::Relaxed);
        inner.tail.index.store(0, Ordering::Relaxed);

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
            self.push_internal(init());
        }

        cap
    }

    pub fn as_vec(&self) -> Vec<T> {
        let _coord = self.wait_exclusive();
        let mut out = Vec::with_capacity(self.len());
        while let Some(value) = self.pop_internal() {
            out.push(value);
        }
        unsafe {
            self.reclaim_retired();
        }
        out
    }
}

impl<T: PartialEq> AtomicVec<T> {
    pub fn index_of(&self, value: &T) -> Option<usize> {
        let _coord = self.wait_exclusive();
        let result = self.index_of_inner(value);
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
        let _coord = self.wait_exclusive();
        let result = self.find_inner(predicate);
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
        let _coord = self.wait_exclusive();
        self.for_each_inner(f);
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
        let _coord = self.wait_exclusive();
        let result = self.fold_inner(init, f);
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
        let _coord = self.wait_exclusive();
        let result = self.reduce_inner(f);
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
        let _coord = self.wait_exclusive();
        let result = self.get_inner(index);
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

            let logical = (start_idx & BLOCK_CAP_MASK) + index;
            let adjusted_block_idx = (start_idx >> BLOCK_SHIFT) + logical / (BLOCK_CAP - 1);
            let final_offset = logical % (BLOCK_CAP - 1);
            let block = self.get_block_at(adjusted_block_idx, start_idx >> BLOCK_SHIFT);
            if block.is_null() {
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
        let logical = |physical: usize| {
            (physical >> BLOCK_SHIFT) * (BLOCK_CAP - 1)
                + (physical & BLOCK_CAP_MASK).min(BLOCK_CAP - 1)
        };
        logical(end) - logical(start)
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
        let _coord = self.wait_exclusive();
        let result = self.swap_inner(i, j);
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
        let logical = (start_idx & BLOCK_CAP_MASK) + index;
        let target_block = (start_idx >> BLOCK_SHIFT) + logical / (BLOCK_CAP - 1);
        let offset = logical % (BLOCK_CAP - 1);
        let block = unsafe { self.get_block_at(target_block, start_idx >> BLOCK_SHIFT) };
        if block.is_null() {
            return ptr::null_mut();
        }

        unsafe { (*block).slots.as_mut_ptr().add(offset) }
    }

    pub fn remove(&self, index: usize) -> Option<T> {
        let _coord = self.wait_exclusive();
        let result = self.remove_inner(index);
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
                self.push_internal(v);
            }
            return None;
        }

        let removed = elements.remove(index);

        for v in elements {
            self.push_internal(v);
        }

        Some(removed)
    }

    pub fn reverse(&self) {
        let _coord = self.wait_exclusive();
        self.reverse_inner();
    }

    fn reverse_inner(&self) {
        let mut elements = Vec::new();
        while let Some(v) = self.pop_internal() {
            elements.push(v);
        }

        for v in elements.into_iter().rev() {
            self.push_internal(v);
        }
    }

    #[inline]
    fn pop_internal(&self) -> Option<T> {
        self.pop_raw().0
    }

    #[inline(always)]
    fn pop_raw(&self) -> (Option<T>, bool) {
        unsafe {
            let inner = self.inner();

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
                        return (None, false);
                    }

                    if (head_idx >> BLOCK_SHIFT) != (tail_idx >> BLOCK_SHIFT) {
                        new_head |= HAS_NEXT;
                    }
                }

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

                        let read_count = (*block).counters.fetch_add(1, Ordering::AcqRel) + 1;
                        let reclaim = if read_count == (BLOCK_CAP - 1) as u64 {
                            let retired = self.retire(block);
                            retired >= 64 && retired % 64 == 0
                        } else {
                            false
                        };
                        return (Some(value), reclaim);
                    }
                    Err(h) => {
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
        crate::core::increment_ref_count(&self.inner().ref_count);
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
            let mut owned = Vec::new();
            let mut block = inner.head.block.load(Ordering::Relaxed);
            while !block.is_null() {
                let next = (*block).next.load(Ordering::Relaxed);
                owned.push(Box::from_raw(block));
                block = next;
            }
            for block in inner.free_list.drain_all() {
                owned.push(Box::from_raw(block));
            }
            let mut block = inner.retired.load(Ordering::Relaxed);
            while !block.is_null() {
                let next = (*block).next.load(Ordering::Relaxed);
                owned.push(Box::from_raw(block));
                block = next;
            }
            drop(Box::from_raw(self.inner.cast_mut()));
            drop(owned);
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
