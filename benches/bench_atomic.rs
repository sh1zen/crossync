use crossync::atomic::{Atomic, AtomicCell};
use std::hint::black_box;
use std::thread;
use std::time::Instant;

pub fn run() {
    const N: usize = 1_000_000;
    const THREADS: usize = 4;
    println!("\nBenching Atomic and AtomicCell\n");
    let atomic = Atomic::new(1usize);
    let cell = AtomicCell::new(1usize);
    let cases: [(&str, &dyn Fn()); 6] = [
        ("Atomic load_copy", &|| {
            black_box(atomic.load_copy());
        }),
        ("Atomic with", &|| {
            atomic.with(|v| {
                black_box(*v);
            });
        }),
        ("Atomic fetch_add", &|| {
            black_box(atomic.fetch_add(1));
        }),
        ("AtomicCell get", &|| {
            black_box(*cell.get());
        }),
        ("AtomicCell get_mut", &|| {
            *cell.get_mut() += 1;
        }),
        ("AtomicCell store", &|| {
            cell.store(black_box(1));
        }),
    ];
    for (name, operation) in cases {
        let start = Instant::now();
        for _ in 0..N {
            operation();
        }
        println!("{name}: {:.2?}", start.elapsed());
    }
    for write in [false, true] {
        let start = Instant::now();
        thread::scope(|scope| {
            for _ in 0..THREADS {
                let atomic = &atomic;
                scope.spawn(move || {
                    for _ in 0..N / THREADS {
                        if write {
                            black_box(atomic.fetch_add(1));
                        } else {
                            atomic.with(|v| {
                                black_box(*v);
                            });
                        }
                    }
                });
            }
        });
        println!(
            "Atomic {} ({THREADS} threads): {:.2?}",
            if write { "fetch_add" } else { "with" },
            start.elapsed()
        );
        let start = Instant::now();
        thread::scope(|scope| {
            for _ in 0..THREADS {
                let cell = &cell;
                scope.spawn(move || {
                    for _ in 0..N / THREADS {
                        if write {
                            *cell.get_mut() += 1;
                        } else {
                            black_box(*cell.get());
                        }
                    }
                });
            }
        });
        println!(
            "AtomicCell {} ({THREADS} threads): {:.2?}",
            if write { "get_mut" } else { "get" },
            start.elapsed()
        );
    }
}
