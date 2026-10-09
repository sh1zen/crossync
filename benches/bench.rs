mod bench_array;
use criterion::criterion_main;

mod bench_hashmap;
mod bench_vec;
mod bench_spincell;
mod bench_atomic;


fn bencher() {
    println!("====== Benchmark Suite ======");

    bench_hashmap::run();
    bench_vec::run();
    bench_array::run();
    bench_spincell::run();
    bench_atomic::run();

    println!("======================");
}

criterion_main!(bencher);
