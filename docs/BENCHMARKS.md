# Benchmarks — carta vs pandoc

**Reference machine:** Apple M1 Pro (10 cores), 16 GB RAM, macOS 26.5 (arm64)
**carta:** commit `3be766a`, release build (`cargo build --release`)
**pandoc:** 3.10
**Driver:** hyperfine 1.20.0, warmup 3, 12 runs

## Headline

- **~14–26× faster** end-to-end across formats and sizes; up to **~35×** on individual reader/writer surfaces.
- **~38× smaller** binary (4.7 MB vs 179.8 MB).
- **~4–28× less peak memory**.

## How to read this

Both tools run with identical `-f/-t` flags; pandoc is configured so both tools produce equivalent output and do equivalent work. Times are wall-clock end-to-end (process start included). `speedup` = pandoc mean ÷ carta mean. `MB/s` is carta throughput over the actual input size. RSS is peak resident memory from a single `/usr/bin/time` run.

## reader — commonmark → json

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     3.67 ms ± 0.45  |    42.51 ms ± 1.32  |   11.6x |        3.5 |    4.0 MB |   107.8 MB |
| 101 KB |     5.96 ms ± 0.24  |    88.73 ms ± 2.89  |   14.9x |       16.6 |    7.6 MB |   121.8 MB |
| 1 MB   |    35.73 ms ± 0.44  |   520.80 ms ± 6.27  |   14.6x |       28.0 |   50.5 MB |   225.8 MB |

## reader — html → json

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 14 KB  |     2.87 ms ± 0.19  |    54.64 ms ± 2.92  |   19.0x |        4.9 |    3.9 MB |   108.8 MB |
| 112 KB |     5.50 ms ± 0.09  |   168.30 ms ± 2.82  |   30.6x |       19.8 |    8.0 MB |   133.7 MB |
| 1 MB   |    33.90 ms ± 1.24  |  1194.45 ms ± 9.49  |   35.2x |       32.6 |   51.1 MB |   428.8 MB |

## writer — json → html

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.41 ms ± 0.24  |    29.41 ms ± 0.96  |   12.2x |        5.0 |    2.8 MB |    41.2 MB |
| 113 KB |     2.69 ms ± 0.13  |    31.99 ms ± 4.86  |   11.9x |       41.0 |    3.2 MB |    61.8 MB |
| 1 MB   |     4.98 ms ± 0.10  |    99.19 ms ± 3.64  |   19.9x |      224.0 |    5.8 MB |   122.9 MB |

## writer — json → latex

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.48 ms ± 0.25  |    29.77 ms ± 1.17  |   12.0x |        4.9 |    2.9 MB |    40.3 MB |
| 113 KB |     2.92 ms ± 0.28  |    29.69 ms ± 0.95  |   10.2x |       37.6 |    3.6 MB |    44.9 MB |
| 1 MB   |     5.69 ms ± 0.19  |    94.33 ms ± 3.82  |   16.6x |      196.2 |    6.3 MB |   122.4 MB |

## writer — json → rst

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.37 ms ± 0.05  |    29.64 ms ± 1.15  |   12.5x |        5.1 |    3.0 MB |    39.5 MB |
| 113 KB |     2.87 ms ± 0.19  |    30.20 ms ± 0.86  |   10.5x |       38.3 |    3.5 MB |    41.2 MB |
| 1 MB   |     6.15 ms ± 0.40  |    89.88 ms ± 2.82  |   14.6x |      181.5 |    6.0 MB |   122.3 MB |

## writer — json → plain

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.45 ms ± 0.24  |    30.30 ms ± 0.85  |   12.3x |        4.9 |    2.9 MB |    39.8 MB |
| 113 KB |     2.98 ms ± 0.31  |    30.12 ms ± 0.95  |   10.1x |       36.9 |    3.5 MB |    62.5 MB |
| 1 MB   |     5.63 ms ± 0.21  |    94.40 ms ± 4.60  |   16.8x |      198.0 |    6.0 MB |   122.6 MB |

## writer — json → commonmark

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.49 ms ± 0.28  |    30.15 ms ± 0.87  |   12.1x |        4.8 |    3.0 MB |    41.7 MB |
| 113 KB |     2.83 ms ± 0.21  |    42.39 ms ± 1.57  |   15.0x |       38.9 |    3.6 MB |    63.3 MB |
| 1 MB   |     5.55 ms ± 0.14  |   182.62 ms ± 4.43  |   32.9x |      201.1 |    6.2 MB |   123.5 MB |

## writer — json → mediawiki

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.44 ms ± 0.21  |    30.45 ms ± 0.74  |   12.5x |        4.9 |    2.9 MB |    39.6 MB |
| 113 KB |     2.79 ms ± 0.19  |    30.44 ms ± 1.01  |   10.9x |       39.5 |    3.5 MB |    43.1 MB |
| 1 MB   |     5.85 ms ± 0.75  |    90.90 ms ± 3.39  |   15.5x |      190.7 |    6.1 MB |   122.2 MB |

## writer — json → native

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.48 ms ± 0.19  |    29.44 ms ± 0.97  |   11.9x |        4.8 |    2.8 MB |    38.9 MB |
| 113 KB |     3.01 ms ± 0.21  |    36.73 ms ± 5.26  |   12.2x |       36.6 |    3.6 MB |    42.7 MB |
| 1 MB   |     6.42 ms ± 0.11  |   166.04 ms ± 4.08  |   25.9x |      173.7 |    8.5 MB |   143.1 MB |

## writer — json → json

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.47 ms ± 0.28  |    29.58 ms ± 1.10  |   12.0x |        4.9 |    2.8 MB |    38.8 MB |
| 113 KB |     2.60 ms ± 0.05  |    30.24 ms ± 0.73  |   11.7x |       42.4 |    3.1 MB |    40.0 MB |
| 1 MB   |     4.79 ms ± 0.12  |    82.97 ms ± 4.30  |   17.3x |      232.9 |    5.7 MB |   121.9 MB |

## e2e — commonmark → html

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     2.97 ms ± 0.15  |    53.05 ms ± 1.90  |   17.9x |        4.3 |    4.0 MB |   108.7 MB |
| 101 KB |     6.07 ms ± 0.31  |   127.86 ms ± 4.78  |   21.1x |       16.3 |    7.6 MB |   122.8 MB |
| 1 MB   |    37.35 ms ± 1.15  |   957.43 ms ± 17.07 |   25.6x |       26.8 |   44.1 MB |   240.8 MB |

## e2e — commonmark → latex

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     3.05 ms ± 0.04  |    52.09 ms ± 3.47  |   17.1x |        4.2 |    4.3 MB |   108.2 MB |
| 101 KB |     6.95 ms ± 0.26  |   122.89 ms ± 4.19  |   17.7x |       14.2 |    8.0 MB |   122.2 MB |
| 1 MB   |    44.24 ms ± 0.59  |   940.83 ms ± 27.54 |   21.3x |       22.6 |   45.2 MB |   259.3 MB |

## e2e — commonmark → rst

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     3.22 ms ± 0.22  |    44.63 ms ± 3.66  |   13.9x |        4.0 |    4.3 MB |   106.9 MB |
| 101 KB |     7.14 ms ± 0.19  |   111.38 ms ± 3.09  |   15.6x |       13.8 |    7.8 MB |   121.9 MB |
| 1 MB   |    47.41 ms ± 0.86  |   820.76 ms ± 34.77 |   17.3x |       21.1 |   43.3 MB |   237.0 MB |

## e2e — commonmark → json

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     3.04 ms ± 0.21  |    43.26 ms ± 3.45  |   14.3x |        4.2 |    4.0 MB |   107.8 MB |
| 101 KB |     5.81 ms ± 0.16  |    91.09 ms ± 5.51  |   15.7x |       17.0 |    7.7 MB |   121.8 MB |
| 1 MB   |    38.67 ms ± 1.08  |   540.18 ms ± 10.03 |   14.0x |       25.9 |   51.3 MB |   225.8 MB |

## startup — commonmark → html (near-empty input)

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 27 B   |     2.46 ms ± 0.06  |    30.68 ms ± 1.82  |   12.5x |        0.0 |    2.8 MB |    41.0 MB |

## startup — commonmark → json (near-empty input)

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 27 B   |     2.53 ms ± 0.31  |    29.34 ms ± 1.59  |   11.6x |        0.0 |    2.8 MB |    39.4 MB |

## binary size

| binary | size       | ratio |
|--------|------------|-------|
| carta  |     4.7 MB |  1.0x |
| pandoc |   179.8 MB |   38x |
