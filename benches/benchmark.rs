use criterion::{criterion_group, criterion_main, Criterion, BenchmarkId};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use text_correction_utils::edit_distance::{edit_distance, edit_operations};
use text_correction_utils::text::{clean, match_words, word_boundaries};
use text_correction_utils::tokenization::{tokenizer, TokenizerType, BOS, EOS};

const INPUT_SIZES: [usize; 4] = [16, 32, 64, 128];

fn bench_edit_distance(c: &mut Criterion) {
    let mut group = c.benchmark_group("edit_distance");
    let mut rng = ChaCha8Rng::seed_from_u64(22);
    for (idx, from_size) in INPUT_SIZES
        .iter()
        .take(INPUT_SIZES.len() - 1)
        .enumerate() {
        for to_size in INPUT_SIZES
            .iter()
            .skip(idx + 1) {
            let from_str: String = (&mut rng)
                .sample_iter::<char, _>(rand::distributions::Standard)
                .take(*from_size)
                .collect();
            let to_str: String = (&mut rng)
                .sample_iter::<char, _>(rand::distributions::Standard)
                .take(*to_size)
                .collect();
            group.bench_with_input(
                BenchmarkId::new(
                    "edit_distance",
                    format!("{} -> {}", from_size, to_size),
                ),
                &(from_str.as_str(), to_str.as_str()),
                |b, &(from, to)| {
                    b.iter(
                        || edit_distance(
                            from,
                            to,
                            true,
                            true,
                            false,
                        )
                    );
                },
            );
            group.bench_with_input(
                BenchmarkId::new(
                    "edit_operations",
                    format!("{} -> {}", from_size, to_size),
                ),
                &(from_str.as_str(), to_str.as_str()),
                |b, &(from, to)| {
                    b.iter(
                        || edit_operations(
                            from,
                            to,
                            true,
                            true,
                            false,
                        )
                    );
                },
            );
        }
    }
    group.finish();
}

fn bench_text(c: &mut Criterion) {
    let mut group = c.benchmark_group("text");
    let mut rng = ChaCha8Rng::seed_from_u64(22);
    for size in INPUT_SIZES.iter() {
        let mut str: String = (&mut rng)
            .sample_iter::<char, _>(rand::distributions::Standard)
            .take(*size)
            .collect();
        for _ in 0..size / 4 {
            let possible_insert_indices: Vec<usize> = (0..=str.len())
                .filter(|i| str.is_char_boundary(*i))
                .collect();
            if possible_insert_indices.is_empty() { continue; }
            let insert_idx = possible_insert_indices[rng.gen_range(0..possible_insert_indices.len())];
            str.insert(insert_idx, ' ');
        }
        group.bench_with_input(
            BenchmarkId::new(
                "clean",
                format!("{}", size),
            ),
            str.as_str(),
            |b, str| {
                b.iter(|| clean(str));
            },
        );
        group.bench_with_input(
            BenchmarkId::new(
                "word_boundaries",
                format!("{}", size),
            ),
            str.as_str(),
            |b, str| {
                b.iter(|| word_boundaries(str, true));
            },
        );
        group.bench_with_input(
            BenchmarkId::new(
                "match_words",
                format!("{}", size),
            ),
            str.as_str(),
            |b, str| {
                b.iter(|| match_words(str, str, false));
            },
        );
    }
}

fn bench_tokenizer(c: &mut Criterion) {
    let mut group = c.benchmark_group("tokenizer");
    let mut rng = ChaCha8Rng::seed_from_u64(22);
    let fx: Vec<String> = vec!["test".to_string()];
    let char_tok = tokenizer(TokenizerType::Character(fx.clone(), fx.clone()));
    let byte_tok = tokenizer(TokenizerType::Byte(fx.clone(), fx.clone()));
    for size in INPUT_SIZES.iter() {
        let str: String = (&mut rng)
            .sample_iter::<char, _>(rand::distributions::Standard)
            .take(*size)
            .collect();
        group.bench_with_input(
            BenchmarkId::new(
                "char",
                format!("{}", size),
            ),
            str.as_str(),
            |b, str| {
                b.iter(|| char_tok.tokenize(str));
            },
        );
        group.bench_with_input(
            BenchmarkId::new(
                "byte",
                format!("{}", size),
            ),
            str.as_str(),
            |b, str| {
                b.iter(|| byte_tok.tokenize(str));
            },
        );
    }
}

criterion_group!(
    benches,
    bench_edit_distance,
    bench_text,
    bench_tokenizer
);
criterion_main!(benches);
