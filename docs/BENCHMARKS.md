# Benchmarks — carta vs pandoc

**Reference machine:** Apple M1 Pro (10 cores), 16 GB RAM, macOS 26.5 (arm64)
**carta:** commit `d5db473`, release build (`cargo build --release`)
**pandoc:** 3.10
**Driver:** hyperfine 1.20.0, warmup 3, 12 runs

## Headline

- **~10–18× faster** end-to-end across formats and sizes; up to **~30×** on individual reader/writer surfaces.
- **~46× smaller** binary (3.9 MB vs 179.8 MB).
- **~2–16× less peak memory**.

## How to read this

Both tools run with identical `-f/-t` flags; pandoc is configured so both tools produce equivalent output and do equivalent work. Times are wall-clock end-to-end (process start included). `speedup` = pandoc mean ÷ carta mean. `MB/s` is carta throughput over the actual input size. RSS is peak resident memory from a single `/usr/bin/time` run.

## reader — commonmark → json

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     3.50 ms ± 0.30  |    43.60 ms ± 1.15  |   12.4x |        3.6 |    4.5 MB |   107.8 MB |
| 101 KB |     7.53 ms ± 0.28  |    93.05 ms ± 1.87  |   12.4x |       13.1 |   12.4 MB |   121.8 MB |
| 1 MB   |    53.51 ms ± 1.05  |   543.00 ms ± 10.60 |   10.1x |       18.7 |   95.9 MB |   225.8 MB |

## reader — html → json

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 14 KB  |     2.84 ms ± 0.17  |    55.45 ms ± 1.49  |   19.5x |        4.9 |    4.3 MB |   108.8 MB |
| 112 KB |     7.04 ms ± 0.27  |   175.60 ms ± 4.99  |   24.9x |       15.5 |   12.6 MB |   133.8 MB |
| 1 MB   |    81.37 ms ± 1.25  |  1251.22 ms ± 17.34 |   15.4x |       13.6 |   97.7 MB |   428.8 MB |

## writer — json → html

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.18 ms ± 0.15  |    30.67 ms ± 0.22  |   14.1x |        5.5 |    2.6 MB |    41.2 MB |
| 113 KB |     2.52 ms ± 0.03  |    33.53 ms ± 4.25  |   13.3x |       43.7 |    3.2 MB |    61.8 MB |
| 1 MB   |     5.71 ms ± 0.11  |   104.37 ms ± 3.74  |   18.3x |      195.3 |    7.5 MB |   122.9 MB |

## writer — json → latex

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.38 ms ± 0.28  |    31.42 ms ± 0.89  |   13.2x |        5.1 |    2.7 MB |    40.3 MB |
| 113 KB |     2.76 ms ± 0.20  |    31.40 ms ± 0.99  |   11.4x |       39.9 |    3.4 MB |    44.9 MB |
| 1 MB   |     6.05 ms ± 0.17  |   104.18 ms ± 4.51  |   17.2x |      184.5 |    7.9 MB |   122.5 MB |

## writer — json → rst

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.31 ms ± 0.14  |    30.79 ms ± 1.16  |   13.4x |        5.2 |    2.9 MB |    39.6 MB |
| 113 KB |     3.03 ms ± 0.28  |    31.36 ms ± 1.07  |   10.4x |       36.4 |    3.5 MB |    41.2 MB |
| 1 MB   |     7.53 ms ± 0.78  |    93.72 ms ± 1.77  |   12.5x |      148.2 |    7.6 MB |   122.3 MB |

## writer — json → plain

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.36 ms ± 0.19  |    31.19 ms ± 1.06  |   13.2x |        5.1 |    2.8 MB |    39.8 MB |
| 113 KB |     2.77 ms ± 0.19  |    31.04 ms ± 0.98  |   11.2x |       39.8 |    3.5 MB |    62.5 MB |
| 1 MB   |     5.74 ms ± 0.05  |    97.01 ms ± 5.94  |   16.9x |      194.3 |    7.7 MB |   122.7 MB |

## writer — json → commonmark

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.15 ms ± 0.05  |    30.42 ms ± 0.48  |   14.2x |        5.6 |    2.8 MB |    41.8 MB |
| 113 KB |     2.90 ms ± 0.19  |    43.30 ms ± 1.19  |   14.9x |       38.0 |    3.5 MB |    63.3 MB |
| 1 MB   |     6.22 ms ± 0.22  |   187.66 ms ± 4.99  |   30.2x |      179.3 |    7.9 MB |   123.5 MB |

## writer — json → mediawiki

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.77 ms ± 0.29  |    31.87 ms ± 1.11  |   11.5x |        4.4 |    2.7 MB |    39.5 MB |
| 113 KB |     2.97 ms ± 0.24  |    31.56 ms ± 1.03  |   10.6x |       37.1 |    3.4 MB |    43.1 MB |
| 1 MB   |     5.94 ms ± 0.12  |    92.90 ms ± 1.56  |   15.6x |      187.9 |    7.6 MB |   122.2 MB |

## writer — json → native

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.33 ms ± 0.24  |    30.28 ms ± 1.02  |   13.0x |        5.2 |    2.7 MB |    39.0 MB |
| 113 KB |     2.79 ms ± 0.20  |    34.41 ms ± 5.10  |   12.3x |       39.5 |    3.6 MB |    42.8 MB |
| 1 MB   |     6.72 ms ± 0.16  |   169.75 ms ± 5.20  |   25.3x |      166.0 |   10.2 MB |   143.1 MB |

## writer — json → json

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.31 ms ± 0.26  |    30.23 ms ± 0.94  |   13.1x |        5.2 |    2.6 MB |    38.8 MB |
| 113 KB |     2.64 ms ± 0.23  |    30.74 ms ± 0.67  |   11.7x |       41.8 |    3.1 MB |    40.0 MB |
| 1 MB   |     5.41 ms ± 0.25  |    82.28 ms ± 4.53  |   15.2x |      206.1 |    7.3 MB |   121.9 MB |

## e2e — commonmark → html

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     3.19 ms ± 0.19  |    56.19 ms ± 1.27  |   17.6x |        4.0 |    4.5 MB |   108.7 MB |
| 101 KB |     8.98 ms ± 0.30  |   131.55 ms ± 3.90  |   14.6x |       11.0 |   11.9 MB |   122.7 MB |
| 1 MB   |    63.58 ms ± 0.99  |   992.18 ms ± 15.99 |   15.6x |       15.7 |   90.5 MB |   240.8 MB |

## e2e — commonmark → latex

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     3.53 ms ± 0.23  |    56.41 ms ± 1.13  |   16.0x |        3.6 |    4.8 MB |   108.2 MB |
| 101 KB |     8.40 ms ± 0.26  |   125.52 ms ± 5.08  |   15.0x |       11.8 |   12.2 MB |   122.2 MB |
| 1 MB   |    60.69 ms ± 0.98  |   930.51 ms ± 21.45 |   15.3x |       16.5 |   89.7 MB |   259.3 MB |

## e2e — commonmark → rst

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     3.22 ms ± 0.04  |    46.44 ms ± 4.77  |   14.4x |        4.0 |    4.8 MB |   106.9 MB |
| 101 KB |     9.33 ms ± 0.17  |   114.58 ms ± 4.77  |   12.3x |       10.6 |   12.1 MB |   121.9 MB |
| 1 MB   |    73.24 ms ± 1.13  |   808.31 ms ± 11.33 |   11.0x |       13.7 |   89.4 MB |   237.0 MB |

## e2e — commonmark → json

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     3.66 ms ± 0.23  |    45.75 ms ± 3.51  |   12.5x |        3.5 |    4.5 MB |   107.8 MB |
| 101 KB |     7.97 ms ± 0.33  |    94.23 ms ± 1.36  |   11.8x |       12.4 |   12.3 MB |   121.8 MB |
| 1 MB   |    52.89 ms ± 0.77  |   530.47 ms ± 9.23  |   10.0x |       18.9 |   95.7 MB |   225.8 MB |

## startup — commonmark → html (near-empty input)

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 27 B   |     2.33 ms ± 0.25  |    30.53 ms ± 0.34  |   13.1x |        0.0 |    2.6 MB |    41.0 MB |

## startup — commonmark → json (near-empty input)

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 27 B   |     2.10 ms ± 0.05  |    29.96 ms ± 0.76  |   14.3x |        0.0 |    2.6 MB |    39.4 MB |

## binary size

| binary | size       | ratio |
|--------|------------|-------|
| carta  |     3.9 MB |  1.0x |
| pandoc |   179.8 MB |   46x |
