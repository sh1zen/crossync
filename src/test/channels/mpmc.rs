#[cfg(test)]
mod tests_mpmc {
    use crate::channels::mpmc::Mpmc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::{Duration, Instant};

    // ==================== CONSTRUCTION ====================

    #[test]
    fn test_new_unbounded() {
        let chan: Mpmc<i32> = Mpmc::new();
        assert!(chan.try_recv().is_none());
    }

    #[test]
    fn test_bounded() {
        let chan: Mpmc<i32> = Mpmc::bounded(5);
        assert!(chan.try_recv().is_none());
    }

    #[test]
    fn test_bounded_zero_is_unbounded() {
        let chan: Mpmc<i32> = Mpmc::bounded(0);
        // Dovrebbe comportarsi come unbounded
        for i in 0..100 {
            assert!(chan.send(i).is_ok());
        }
    }

    // ==================== SEND / RECV BASIC ====================

    #[test]
    fn test_send_try_recv() {
        let chan = Mpmc::new();

        assert!(chan.try_recv().is_none());

        chan.send(42).unwrap();
        assert_eq!(chan.try_recv(), Some(42));
        assert!(chan.try_recv().is_none());
    }

    #[test]
    fn test_send_recv_blocking() {
        let chan = Mpmc::new();

        chan.send(100).unwrap();
        assert_eq!(chan.recv(), Some(100));
    }

    #[test]
    fn test_fifo_order() {
        let chan = Mpmc::new();

        for i in 0..10 {
            chan.send(i).unwrap();
        }

        for i in 0..10 {
            assert_eq!(chan.try_recv(), Some(i));
        }
    }

    #[test]
    fn test_multiple_values() {
        let chan = Mpmc::new();

        chan.send("hello").unwrap();
        chan.send("world").unwrap();
        chan.send("!").unwrap();

        assert_eq!(chan.try_recv(), Some("hello"));
        assert_eq!(chan.try_recv(), Some("world"));
        assert_eq!(chan.try_recv(), Some("!"));
        assert!(chan.try_recv().is_none());
    }

    // ==================== BOUNDED CHANNEL ====================

    #[test]
    fn test_bounded_capacity() {
        let chan = Mpmc::bounded(2);

        assert!(chan.send(1).is_ok());
        assert!(chan.send(2).is_ok());
        // Buffer pieno
        assert!(chan.send(3).is_err());

        // Libera spazio
        assert_eq!(chan.try_recv(), Some(1));

        // Ora c'è spazio
        assert!(chan.send(3).is_ok());

        assert_eq!(chan.try_recv(), Some(2));
        assert_eq!(chan.try_recv(), Some(3));
        assert!(chan.try_recv().is_none());
    }

    #[test]
    fn test_bounded_returns_value_on_error() {
        let chan = Mpmc::bounded(1);
        chan.send(1).unwrap();

        let result = chan.send(2);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), 2);
    }

    // ==================== CLOSE ====================

    #[test]
    fn test_close_rejects_send() {
        let chan = Mpmc::new();
        chan.send(10).unwrap();

        chan.close();

        // Send dopo close fallisce
        assert!(chan.send(20).is_err());
    }

    #[test]
    fn test_close_allows_drain() {
        let chan = Mpmc::new();
        chan.send(1).unwrap();
        chan.send(2).unwrap();
        chan.send(3).unwrap();

        chan.close();

        // Può ancora ricevere elementi esistenti
        assert_eq!(chan.try_recv(), Some(1));
        assert_eq!(chan.try_recv(), Some(2));
        assert_eq!(chan.try_recv(), Some(3));
        assert!(chan.try_recv().is_none());
    }

    #[test]
    fn test_recv_on_closed_empty() {
        let chan: Mpmc<i32> = Mpmc::new();
        chan.close();

        // recv() su canale chiuso e vuoto ritorna None immediatamente
        let start = Instant::now();
        assert_eq!(chan.recv(), None);
        assert!(start.elapsed() < Duration::from_millis(100));
    }

    #[test]
    fn test_close_wakes_waiting_receivers() {
        let chan: Mpmc<i32> = Mpmc::new();
        let woken = Arc::new(AtomicUsize::new(0));

        let w = woken.clone();
        let c = chan.clone();
        let handle = thread::spawn(move || {
            let result = c.recv(); // Blocca
            w.store(1, Ordering::SeqCst);
            result
        });

        thread::sleep(Duration::from_millis(50));
        assert_eq!(woken.load(Ordering::SeqCst), 0);

        chan.close();

        let result = handle.join().unwrap();
        assert_eq!(result, None);
        assert_eq!(woken.load(Ordering::SeqCst), 1);
    }

    // ==================== CLONE & REFCOUNT ====================

    #[test]
    fn test_clone_shares_channel() {
        let chan = Mpmc::new();
        let chan2 = chan.clone();
        let chan3 = chan2.clone();

        chan.send(100).unwrap();
        assert_eq!(chan2.try_recv(), Some(100));
        assert!(chan3.try_recv().is_none());

        chan3.send(200).unwrap();
        assert_eq!(chan.try_recv(), Some(200));
    }

    #[test]
    fn test_clone_drop_partial() {
        let chan = Mpmc::new();
        let c1 = chan.clone();
        let c2 = chan.clone();

        chan.send(42).unwrap();

        drop(c1);
        drop(c2);

        // Originale ancora funziona
        assert_eq!(chan.try_recv(), Some(42));
    }

    // ==================== CONCURRENT - BASIC ====================

    #[test]
    fn test_recv_blocks_until_send() {
        let chan = Mpmc::new();
        let received = Arc::new(AtomicUsize::new(0));

        let r = received.clone();
        let c = chan.clone();
        let handle = thread::spawn(move || {
            let val = c.recv();
            r.store(1, Ordering::SeqCst);
            val
        });

        thread::sleep(Duration::from_millis(50));
        assert_eq!(received.load(Ordering::SeqCst), 0, "recv returned too early");

        chan.send(99).unwrap();

        let result = handle.join().unwrap();
        assert_eq!(result, Some(99));
        assert_eq!(received.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn test_single_producer_single_consumer() {
        let chan = Mpmc::new();
        let count = 100;

        let c = chan.clone();
        let producer = thread::spawn(move || {
            for i in 0..count {
                c.send(i).unwrap();
            }
        });

        let c = chan.clone();
        let consumer = thread::spawn(move || {
            let mut received = Vec::new();
            for _ in 0..count {
                if let Some(v) = c.recv() {
                    received.push(v);
                }
            }
            received
        });

        producer.join().unwrap();
        let received = consumer.join().unwrap();

        assert_eq!(received.len(), count);
        for i in 0..count {
            assert_eq!(received[i], i);
        }
    }

    // ==================== CONCURRENT - MPMC ====================

    #[test]
    fn test_multiple_producers() {
        let chan = Mpmc::new();
        let barrier = Arc::new(Barrier::new(4));

        let handles: Vec<_> = (0..4)
            .map(|id| {
                let c = chan.clone();
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                    for i in 0..25 {
                        c.send(id * 100 + i).unwrap();
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        let mut count = 0;
        while chan.try_recv().is_some() {
            count += 1;
        }

        assert_eq!(count, 100); // 4 * 25
    }

    #[test]
    fn test_multiple_consumers() {
        let chan = Mpmc::new();
        let total = 100;

        // Pre-popola il canale
        for i in 0..total {
            chan.send(i).unwrap();
        }
        chan.close();

        let received = Arc::new(AtomicUsize::new(0));

        let handles: Vec<_> = (0..4)
            .map(|_| {
                let c = chan.clone();
                let r = received.clone();
                thread::spawn(move || {
                    while let Some(_) = c.recv() {
                        r.fetch_add(1, Ordering::SeqCst);
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(received.load(Ordering::SeqCst), total);
    }

    #[test]
    fn test_mpmc_full() {
        let chan = Mpmc::new();
        let barrier = Arc::new(Barrier::new(7)); // 3 prod + 4 cons
        let total_messages = 30; // 3 * 10

        // 3 producers
        let producers: Vec<_> = (0..3)
            .map(|id| {
                let c = chan.clone();
                let b = barrier.clone();
                thread::spawn(move || {
                    b.wait();
                    for i in 0..10 {
                        c.send((id, i)).unwrap();
                    }
                })
            })
            .collect();

        // 4 consumers
        let received = Arc::new(AtomicUsize::new(0));
        let consumers: Vec<_> = (0..4)
            .map(|_| {
                let c = chan.clone();
                let b = barrier.clone();
                let r = received.clone();
                thread::spawn(move || {
                    b.wait();
                    while let Some(_) = c.recv() {
                        let count = r.fetch_add(1, Ordering::SeqCst) + 1;
                        if count >= total_messages {
                            break;
                        }
                    }
                })
            })
            .collect();

        for p in producers {
            p.join().unwrap();
        }

        // Chiudi per far terminare i consumer
        chan.close();

        for c in consumers {
            c.join().unwrap();
        }

        assert_eq!(received.load(Ordering::SeqCst), total_messages);
    }

    #[test]
    fn test_concurrent_send_recv() {
        let chan = Mpmc::new();
        let barrier = Arc::new(Barrier::new(2));

        let c = chan.clone();
        let b = barrier.clone();
        let sender = thread::spawn(move || {
            b.wait();
            for i in 0..50 {
                c.send(i).unwrap();
                thread::yield_now();
            }
        });

        let c = chan.clone();
        let b = barrier.clone();
        let receiver = thread::spawn(move || {
            b.wait();
            let mut received = Vec::new();
            for _ in 0..50 {
                if let Some(v) = c.recv() {
                    received.push(v);
                }
            }
            received
        });

        sender.join().unwrap();
        let received = receiver.join().unwrap();

        assert_eq!(received.len(), 50);
    }

    // ==================== EDGE CASES ====================

    #[test]
    fn test_empty_recv_then_send() {
        let chan = Mpmc::new();

        assert!(chan.try_recv().is_none());
        assert!(chan.try_recv().is_none());

        chan.send(1).unwrap();
        assert_eq!(chan.try_recv(), Some(1));
    }

    #[test]
    fn test_complex_type() {
        let chan = Mpmc::new();

        chan.send(vec![1, 2, 3]).unwrap();
        chan.send(vec![4, 5]).unwrap();

        assert_eq!(chan.try_recv(), Some(vec![1, 2, 3]));
        assert_eq!(chan.try_recv(), Some(vec![4, 5]));
    }

    #[test]
    fn test_string_channel() {
        let chan = Mpmc::new();

        chan.send(String::from("hello")).unwrap();
        chan.send(String::from("world")).unwrap();

        assert_eq!(chan.recv(), Some(String::from("hello")));
        assert_eq!(chan.recv(), Some(String::from("world")));
    }

    #[test]
    fn test_option_type() {
        let chan: Mpmc<Option<i32>> = Mpmc::new();

        chan.send(Some(42)).unwrap();
        chan.send(None).unwrap();
        chan.send(Some(99)).unwrap();

        assert_eq!(chan.try_recv(), Some(Some(42)));
        assert_eq!(chan.try_recv(), Some(None));
        assert_eq!(chan.try_recv(), Some(Some(99)));
    }

    #[test]
    fn test_rapid_send_recv() {
        let chan = Mpmc::new();

        for _ in 0..100 {
            chan.send(42).unwrap();
            assert_eq!(chan.try_recv(), Some(42));
        }
    }

    #[test]
    fn test_bounded_one() {
        let chan = Mpmc::bounded(1);

        assert!(chan.send(1).is_ok());
        assert!(chan.send(2).is_err());

        assert_eq!(chan.try_recv(), Some(1));

        assert!(chan.send(2).is_ok());
        assert_eq!(chan.try_recv(), Some(2));
    }

    // ==================== MEMORY SAFETY ====================

    #[derive(Clone)]
    #[derive(Debug)]
struct DropTracker {
        counter: Arc<AtomicUsize>,
    }

    impl DropTracker {
        fn new(counter: Arc<AtomicUsize>) -> Self {
            counter.fetch_add(1, Ordering::SeqCst);
            Self { counter }
        }
    }

    impl Drop for DropTracker {
        fn drop(&mut self) {
            self.counter.fetch_sub(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn test_no_leak_on_drop() {
        let counter = Arc::new(AtomicUsize::new(0));

        {
            let chan = Mpmc::new();
            for _ in 0..10 {
                chan.send(DropTracker::new(counter.clone())).unwrap();
            }
            assert_eq!(counter.load(Ordering::SeqCst), 10);
        }

        assert_eq!(counter.load(Ordering::SeqCst), 0, "Memory leak detected");
    }

    #[test]
    fn test_no_leak_on_recv() {
        let counter = Arc::new(AtomicUsize::new(0));

        let chan = Mpmc::new();
        for _ in 0..5 {
            chan.send(DropTracker::new(counter.clone())).unwrap();
        }
        assert_eq!(counter.load(Ordering::SeqCst), 5);

        while chan.try_recv().is_some() {}

        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn test_no_leak_bounded_reject() {
        let counter = Arc::new(AtomicUsize::new(0));

        let chan = Mpmc::bounded(1);
        chan.send(DropTracker::new(counter.clone())).unwrap();

        // Questo viene rifiutato
        let rejected = chan.send(DropTracker::new(counter.clone()));
        assert!(rejected.is_err());

        // Il valore rifiutato deve essere droppato
        drop(rejected);
        assert_eq!(counter.load(Ordering::SeqCst), 1);

        drop(chan);
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }
}