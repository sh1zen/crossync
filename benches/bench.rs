use criterion::criterion_main;

mod bench_hashmap;
mod bench_vec;
mod bench_spincell;


fn bencher() {
    println!("====== Benchmark Suite ======");

    bench_hashmap::run();
    bench_vec::run();
    bench_spincell::run();

    println!("======================");
}

criterion_main!(bencher);