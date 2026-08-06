//! `rabench` — Tier-B harness CLI (plan §7.2/§7.3/§4).

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::process::ExitCode;

use rusty_alloc_bench::replay::{self, Arm};
use rusty_alloc_bench::trace::{Op, Reader};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |i: usize| args.get(i).map(String::as_str);
    match arg(0) {
        Some("info") if args.len() == 2 => info(&args[1]),
        Some("gen") if args.len() >= 2 => {
            let n: u64 = arg(2).and_then(|s| s.parse().ok()).unwrap_or(1_000_000);
            let seed: u64 = arg(3).and_then(|s| s.parse().ok()).unwrap_or(0x5EED);
            match replay::generate(&args[1], n, seed) {
                Ok(()) => {
                    println!("wrote {} ({n} churn ops, seed {seed:#x})", args[1]);
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("rabench gen: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("replay") if args.len() >= 3 => {
            let Some(arm) = Arm::parse(&args[2]) else {
                eprintln!("rabench replay: unknown arm '{}' (ra|sys)", args[2]);
                return ExitCode::from(2);
            };
            let check = args.iter().any(|a| a == "--check");
            match replay::replay(&args[1], arm, check) {
                Ok((ops, _)) => {
                    println!(
                        "replayed {ops} ops on {} (check={check}) — G1 CLEAN",
                        args[2]
                    );
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("rabench replay: G1 FAIL: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("malloc-small") => {
            let arm = arg(1).and_then(Arm::parse).unwrap_or(Arm::Rusty);
            let ops: u64 = arg(2).and_then(|s| s.parse().ok()).unwrap_or(20_000_000);
            let seed: u64 = arg(3).and_then(|s| s.parse().ok()).unwrap_or(0x5EED);
            rusty_alloc_bench::kernels::malloc_small(arm, ops, seed);
            ExitCode::SUCCESS
        }
        Some("freepath-probe") => {
            let iters: u64 = arg(1).and_then(|s| s.parse().ok()).unwrap_or(20_000_000);
            rusty_alloc_bench::kernels::freepath_probe(iters);
            ExitCode::SUCCESS
        }
        Some("tls-spike") => {
            let iters: u64 = arg(1).and_then(|s| s.parse().ok()).unwrap_or(100_000_000);
            rusty_alloc_bench::kernels::tls_spike(iters);
            ExitCode::SUCCESS
        }
        Some("larson") => {
            let arm = arg(1).and_then(Arm::parse).unwrap_or(Arm::Rusty);
            let threads: usize = arg(2).and_then(|s| s.parse().ok()).unwrap_or(8);
            let ops: u64 = arg(3).and_then(|s| s.parse().ok()).unwrap_or(2_000_000);
            let seed: u64 = arg(4).and_then(|s| s.parse().ok()).unwrap_or(0x1A25);
            rusty_alloc_bench::kernels::larson(arm, threads, ops, seed);
            ExitCode::SUCCESS
        }
        Some("xmalloc") => {
            let arm = arg(1).and_then(Arm::parse).unwrap_or(Arm::Rusty);
            let pairs: usize = arg(2).and_then(|s| s.parse().ok()).unwrap_or(4);
            let ops: u64 = arg(3).and_then(|s| s.parse().ok()).unwrap_or(2_000_000);
            let seed: u64 = arg(4).and_then(|s| s.parse().ok()).unwrap_or(0x3A11);
            rusty_alloc_bench::kernels::xmalloc(arm, pairs, ops, seed);
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("usage: rabench info <file.ratrace>");
            eprintln!("       rabench gen <file.ratrace> [ops] [seed]");
            eprintln!("       rabench replay <file.ratrace> <ra|sys> [--check]");
            eprintln!("       rabench malloc-small [ra|sys] [ops] [seed]");
            eprintln!("       rabench larson [threads] [ops/thread] [seed]");
            eprintln!("       rabench xmalloc [pairs] [ops/pair] [seed]");
            eprintln!("       rabench tls-spike [iters]");
            ExitCode::from(2)
        }
    }
}

fn info(path: &str) -> ExitCode {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("rabench: cannot open {path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut reader = match Reader::new(BufReader::new(file)) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("rabench: {path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut counts: HashMap<&'static str, u64> = HashMap::new();
    let mut threads = 0u16;
    let mut bytes_requested = 0u64;
    let mut live = 0i64;
    let mut peak_live = 0i64;
    loop {
        match reader.next_record() {
            Ok(Some(r)) => {
                let name = match r.op {
                    Op::Malloc => "malloc",
                    Op::Zalloc => "zalloc",
                    Op::Free => "free",
                    Op::Realloc => "realloc",
                    Op::ThreadStart => "thread_start",
                    Op::ThreadEnd => "thread_end",
                };
                *counts.entry(name).or_insert(0) += 1;
                threads = threads.max(r.thread + 1);
                bytes_requested += r.size;
                match r.op {
                    Op::Malloc | Op::Zalloc => live += 1,
                    Op::Free => live -= 1,
                    _ => {}
                }
                peak_live = peak_live.max(live);
            }
            Ok(None) => break,
            Err(e) => {
                eprintln!("rabench: {path}: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    let total: u64 = counts.values().sum();
    println!("{path}: {total} records, {threads} thread(s)");
    let mut rows: Vec<_> = counts.into_iter().collect();
    rows.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
    for (name, n) in rows {
        println!("  {name:>12}: {n}");
    }
    println!("  bytes requested: {bytes_requested}");
    println!("  peak live blocks: {peak_live}");
    ExitCode::SUCCESS
}
