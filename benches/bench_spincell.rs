use std::collections::HashMap;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Instant;
use crossync::lock_free::SpinCell;

fn benchmark_spinlockcell(
    n_threads: usize,
    n_operations: usize,
    map: Arc<SpinCell<HashMap<usize, usize>>>,
) {
    let barrier = Arc::new(Barrier::new(n_threads));

    // Popola la mappa iniziale
    {
        let mut guard = map.lock_exclusive();
        for i in 0..1000 {
            guard.insert(i, i);
        }
    }

    let start = Instant::now();
    let mut handles = Vec::new();

    for _ in 0..n_threads {
        let map_clone = Arc::clone(&map);
        let barrier_clone = Arc::clone(&barrier);

        let h = thread::spawn(move || {
            // aspetta che tutti i thread siano pronti
            barrier_clone.wait();

            for op_id in 0..n_operations {
                if op_id % 2 == 0 {
                    // scrittura
                    let key = op_id % 1000;
                    let mut guard = map_clone.lock_exclusive();
                    guard.insert(key, op_id);
                } else {
                    // lettura
                    let key = op_id % 1000;
                    let guard = map_clone.lock_shared();
                    let _ = guard.get(&key);
                }
            }
        });

        handles.push(h);
    }

    for h in handles {
        h.join().unwrap();
    }

    let elapsed = start.elapsed();
    println!(
        "{} threads, {} read/write operations each → {:?}",
        n_threads, n_operations, elapsed
    );
}

pub fn run() {
    println!("\nBenching SpinLockCell\n");

    for &threads in &[1, 2, 4, 8, 16] {
        let map = Arc::new(SpinCell::new(HashMap::new()));
        benchmark_spinlockcell(threads, 100_000, map);
    }
}
