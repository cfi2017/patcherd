use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use patcherd::search::{self, Pattern};

// ---------------------------------------------------------------------------
// Naive baseline (old implementation) for comparison
// ---------------------------------------------------------------------------

fn naive_find(haystack: &[u8], needle: &[u8]) -> Vec<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return vec![];
    }
    let mut result = Vec::new();
    let mut i = 0;
    while i + needle.len() <= haystack.len() {
        if haystack[i..i + needle.len()] == *needle {
            result.push(i);
            i += needle.len();
        } else {
            i += 1;
        }
    }
    result
}

fn naive_replace(data: &[u8], find: &[u8], replace: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if i + find.len() <= data.len() && data[i..i + find.len()] == *find {
            result.extend_from_slice(replace);
            i += find.len();
        } else {
            result.push(data[i]);
            i += 1;
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn pat(bytes: &[u8]) -> Vec<Pattern> {
    bytes.iter().map(|&b| Pattern::Byte(b)).collect()
}

fn make_haystack(size: usize, needle: &[u8], spacing: usize) -> Vec<u8> {
    let mut h = vec![0x42u8; size];
    for i in (0..size).step_by(spacing.max(1)) {
        if i + needle.len() <= size {
            h[i..i + needle.len()].copy_from_slice(needle);
        }
    }
    h
}

// ---------------------------------------------------------------------------
// Benchmarks
// ---------------------------------------------------------------------------

fn bench_find_exact(c: &mut Criterion) {
    let needle = [0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE, 0xBA, 0xBE];
    let pattern = pat(&needle);

    let mut group = c.benchmark_group("find_exact");
    for &size in &[1024usize, 64 * 1024, 1024 * 1024, 16 * 1024 * 1024] {
        let h = make_haystack(size, &needle, size / 10);
        group.throughput(Throughput::Bytes(size as u64));

        group.bench_with_input(BenchmarkId::new("naive", size), &h, |b, h| {
            b.iter(|| naive_find(h, &needle));
        });

        group.bench_with_input(BenchmarkId::new("memchr", size), &h, |b, h| {
            b.iter(|| search::find_all(h, &pattern));
        });
    }
    group.finish();
}

fn bench_find_wildcard(c: &mut Criterion) {
    let needle_bytes = [0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE, 0xBA, 0xBE];
    let mut pattern = pat(&needle_bytes);
    pattern[2] = Pattern::Wildcard; // DE AD ?? EF CA FE BA BE
    pattern[5] = Pattern::Wildcard; // DE AD ?? EF CA ?? BA BE

    let mut group = c.benchmark_group("find_wildcard");
    for &size in &[1024usize, 64 * 1024, 1024 * 1024, 16 * 1024 * 1024] {
        let h = make_haystack(size, &needle_bytes, size / 10);
        group.throughput(Throughput::Bytes(size as u64));

        group.bench_with_input(BenchmarkId::new("memchr_anchor", size), &h, |b, h| {
            b.iter(|| search::find_all(h, &pattern));
        });
    }
    group.finish();
}

fn bench_replace(c: &mut Criterion) {
    let needle = [0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE, 0xBA, 0xBE];
    let replace = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
    let pattern = pat(&needle);

    let mut group = c.benchmark_group("replace");
    for &size in &[1024usize, 64 * 1024, 1024 * 1024, 16 * 1024 * 1024] {
        let h = make_haystack(size, &needle, size / 10);
        group.throughput(Throughput::Bytes(size as u64));

        group.bench_with_input(BenchmarkId::new("naive", size), &h, |b, h| {
            b.iter(|| naive_replace(h, &needle, &replace));
        });

        group.bench_with_input(BenchmarkId::new("memchr_streaming", size), &h, |b, h| {
            b.iter(|| search::replace_all(h, &pattern, &replace));
        });
    }
    group.finish();
}

fn bench_needle_size(c: &mut Criterion) {
    let size = 1024 * 1024;
    let mut group = c.benchmark_group("needle_size");
    group.throughput(Throughput::Bytes(size as u64));

    for &nlen in &[2usize, 4, 8, 16, 32, 64] {
        let needle: Vec<u8> = (0xA0..0xA0 + nlen as u8).collect();
        let pattern = pat(&needle);
        let h = make_haystack(size, &needle, size / 10);

        group.bench_with_input(
            BenchmarkId::new("naive", nlen),
            &(&h, &needle),
            |b, (h, n)| {
                b.iter(|| naive_find(h, n));
            },
        );

        group.bench_with_input(
            BenchmarkId::new("memchr", nlen),
            &(&h, &pattern),
            |b, (h, p)| {
                b.iter(|| search::find_all(h, p));
            },
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_find_exact,
    bench_find_wildcard,
    bench_replace,
    bench_needle_size,
);
criterion_main!(benches);
