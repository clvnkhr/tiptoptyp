//! Thread-local allocation observation for deterministic ownership regressions.
//! All allocation and deallocation still delegates to the system allocator.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

thread_local! {
    static ALLOCATED: Cell<Option<usize>> = const { Cell::new(None) };
}

struct CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        record(size);
        unsafe { System.realloc(pointer, layout, size) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn record(size: usize) {
    let _ = ALLOCATED.try_with(|count| {
        if let Some(bytes) = count.get() {
            count.set(Some(bytes.saturating_add(size)));
        }
    });
}

pub(crate) fn allocated<T>(operation: impl FnOnce() -> T) -> (T, usize) {
    ALLOCATED.with(|count| count.set(Some(0)));
    let output = operation();
    let bytes = ALLOCATED.with(|count| count.replace(None).unwrap_or_default());
    (output, bytes)
}
