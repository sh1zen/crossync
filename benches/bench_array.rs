use crossync::atomic::AtomicArray;
use std::thread;
use std::time::Instant;

pub fn run() {
    println!("\nBenching AtomicArray\n");
    const N: usize = 1_000_000;

    // === Single-thread push ===
    let arr = AtomicArray::with_capacity(N);
    let start = Instant::now();
    for i in 0..N {
        let _ = arr.push(i);
    }
    let duration = start.elapsed();
    println!("Single-thread push: {:.2?}", duration);

    // === Single-thread read ===
    let start = Instant::now();
    for i in 0..N {
        let _ = arr.get(i);
    }
    let duration = start.elapsed();
    println!("Single-thread get: {:.2?}", duration);

    // === Single-thread write ===
    let start = Instant::now();
    for i in 0..N {
        if let Some(mut guard) = arr.get_mut(i) {
            *guard = i + 1;
        }
    }
    let duration = start.elapsed();
    println!("Single-thread get_mut: {:.2?}", duration);

    // === Multi-threaded push ===
    let arr = AtomicArray::with_capacity(N);
    let start = Instant::now();
    let mut handles = Vec::new();
    let threads = 12;
    let per_thread = N / threads;

    for t in 0..threads {
        let arr = arr.clone();
        handles.push(thread::spawn(move || {
            for j in 0..per_thread {
                let _ = arr.push(t * per_thread + j);
            }
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    let duration = start.elapsed();
    println!(
        "Multi-threaded push ({} threads): {:.2?}",
        threads, duration
    );

    // === Multi-threaded read ===
    let arr = AtomicArray::init_with(N, || 0usize);
    let start = Instant::now();
    let mut handles = Vec::new();

    for _ in 0..threads {
        let arr = arr.clone();
        handles.push(thread::spawn(move || {
            for i in 0..per_thread {
                let _ = arr.get(i);
            }
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    let duration = start.elapsed();
    println!(
        "Multi-threaded get ({} threads): {:.2?}",
        threads, duration
    );

    // === Multi-threaded read + write (mixed) ===
    let arr = AtomicArray::init_with(N, || 0usize);
    let start = Instant::now();
    let mut handles = Vec::new();

    // Readers
    for _ in 0..threads {
        let arr = arr.clone();
        handles.push(thread::spawn(move || {
            for i in 0..per_thread {
                let _ = arr.get(i);
            }
        }));
    }

    // Writers
    for t in 0..threads {
        let arr = arr.clone();
        handles.push(thread::spawn(move || {
            let start_idx = t * per_thread;
            for i in 0..per_thread {
                if let Some(mut guard) = arr.get_mut(start_idx + i) {
                    *guard = i;
                }
            }
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    let duration = start.elapsed();
    println!(
        "Multi-threaded get + get_mut ({} threads): {:.2?}",
        threads * 2,
        duration
    );

    // === for_each benchmark ===
    let arr = AtomicArray::init_with(N, || 0usize);
    let start = Instant::now();
    let mut sum = 0usize;
    arr.for_each(|v| sum += *v);
    let duration = start.elapsed();
    println!("for_each (sum={}): {:.2?}", sum, duration);

    // === for_each_mut benchmark ===
    let arr = AtomicArray::init_with(N, || 0usize);
    let start = Instant::now();
    arr.for_each_mut(|v| *v += 1);
    let duration = start.elapsed();
    println!("for_each_mut: {:.2?}", duration);

    // === Parallel chunk processing ===
    let arr = AtomicArray::init_with(N, || 0usize);
    let chunks = arr.chunk_indices(threads);
    let start = Instant::now();
    let mut handles = Vec::new();

    for (start_idx, end_idx) in chunks {
        let arr = arr.clone();
        handles.push(thread::spawn(move || {
            for i in start_idx..end_idx {
                if let Some(mut guard) = arr.get_mut(i) {
                    *guard = i;
                }
            }
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    let duration = start.elapsed();
    println!(
        "Parallel chunk write ({} threads): {:.2?}",
        threads, duration
    );
}