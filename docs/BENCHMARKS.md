# Benchmarks — carta vs pandoc

**Reference machine:** Apple M1 Pro (10 cores), 16 GB RAM, macOS 26.5 (arm64)
**carta:** commit `5e110f9`, release build (`cargo build --release`)
**pandoc:** 3.10
**Driver:** hyperfine 1.20.0, warmup 3, 12 runs

## Headline

- **~10–30× faster** end-to-end across formats and sizes.
- **~69× smaller** binary (2.6 MB vs 179.8 MB).
- **~3–14× less peak memory**.

## How to read this

Both tools run with identical `-f/-t` flags; pandoc is configured so both tools produce equivalent output and do equivalent work. Times are wall-clock end-to-end (process start included). `speedup` = pandoc mean ÷ carta mean. `MB/s` is carta throughput over the actual input size. RSS is peak resident memory from a single `/usr/bin/time` run.

## reader — commonmark → json

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     3.32 ms ± 0.21  |    46.24 ms ± 3.03  |   13.9x |        3.8 |    4.0 MB |   107.8 MB |
| 101 KB |     7.95 ms ± 0.65  |    94.85 ms ± 2.85  |   11.9x |       12.4 |   11.4 MB |   121.8 MB |
| 1 MB   |    59.29 ms ± 2.83  |   565.85 ms ± 27.53 |    9.5x |       16.9 |   90.7 MB |   225.8 MB |

## reader — html → json

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 14 KB  |     2.95 ms ± 0.14  |    56.51 ms ± 1.04  |   19.1x |        4.8 |    3.9 MB |   108.8 MB |
| 112 KB |     7.37 ms ± 0.24  |   180.08 ms ± 2.31  |   24.4x |       14.8 |   12.2 MB |   133.8 MB |
| 1 MB   |    87.89 ms ± 4.30  |  1290.49 ms ± 51.57 |   14.7x |       12.6 |   97.4 MB |   428.6 MB |

## writer — json → html

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.43 ms ± 0.19  |    30.67 ms ± 0.88  |   12.6x |        5.0 |    2.5 MB |    41.2 MB |
| 113 KB |     2.69 ms ± 0.10  |    32.27 ms ± 3.22  |   12.0x |       40.9 |    3.1 MB |    61.8 MB |
| 1 MB   |     5.81 ms ± 0.14  |   104.59 ms ± 1.97  |   18.0x |      191.9 |    7.3 MB |   122.9 MB |

## writer — json → latex

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.03 ms ± 0.04  |    30.63 ms ± 0.88  |   15.1x |        5.9 |    2.5 MB |    40.3 MB |
| 113 KB |     2.54 ms ± 0.07  |    30.79 ms ± 0.72  |   12.1x |       43.3 |    3.2 MB |    44.9 MB |
| 1 MB   |     6.02 ms ± 0.20  |   106.91 ms ± 4.19  |   17.8x |      185.2 |    7.7 MB |   122.5 MB |

## writer — json → rst

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.93 ms ± 1.02  |    31.97 ms ± 0.82  |   10.9x |        4.1 |    2.6 MB |    39.5 MB |
| 113 KB |     3.14 ms ± 0.18  |    31.83 ms ± 1.94  |   10.1x |       35.1 |    3.3 MB |    41.2 MB |
| 1 MB   |     6.58 ms ± 0.17  |    96.64 ms ± 5.70  |   14.7x |      169.5 |    7.4 MB |   122.3 MB |

## writer — json → plain

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.57 ms ± 0.32  |    31.30 ms ± 0.58  |   12.2x |        4.7 |    2.5 MB |    39.8 MB |
| 113 KB |     2.82 ms ± 0.31  |    37.52 ms ± 6.68  |   13.3x |       39.0 |    3.2 MB |    62.6 MB |
| 1 MB   |     5.92 ms ± 0.17  |    97.81 ms ± 5.92  |   16.5x |      188.3 |    7.4 MB |   122.7 MB |

## writer — json → commonmark

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.20 ms ± 0.11  |    31.47 ms ± 0.55  |   14.3x |        5.5 |    2.5 MB |    41.8 MB |
| 113 KB |     2.67 ms ± 0.18  |    46.79 ms ± 6.04  |   17.5x |       41.2 |    3.2 MB |    63.4 MB |
| 1 MB   |     6.22 ms ± 0.22  |   192.11 ms ± 3.32  |   30.9x |      179.2 |    7.6 MB |   123.6 MB |

## writer — json → mediawiki

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.39 ms ± 0.13  |    31.41 ms ± 0.68  |   13.1x |        5.0 |    2.5 MB |    39.6 MB |
| 113 KB |     2.63 ms ± 0.10  |    31.76 ms ± 1.41  |   12.1x |       41.9 |    3.2 MB |    43.1 MB |
| 1 MB   |     6.65 ms ± 0.39  |    99.14 ms ± 6.39  |   14.9x |      167.7 |    7.4 MB |   122.2 MB |

## writer — json → native

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.30 ms ± 0.27  |    31.47 ms ± 0.70  |   13.7x |        5.2 |    2.5 MB |    39.0 MB |
| 113 KB |     3.19 ms ± 0.11  |    43.18 ms ± 1.67  |   13.5x |       34.5 |    3.4 MB |    42.8 MB |
| 1 MB   |     7.40 ms ± 0.49  |   178.13 ms ± 5.86  |   24.1x |      150.8 |   10.0 MB |   143.1 MB |

## writer — json → json

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.12 ms ± 0.14  |    31.29 ms ± 0.80  |   14.8x |        5.7 |    2.3 MB |    38.8 MB |
| 113 KB |     2.59 ms ± 0.24  |    31.70 ms ± 1.13  |   12.3x |       42.6 |    2.9 MB |    40.0 MB |
| 1 MB   |     5.62 ms ± 0.25  |    91.64 ms ± 1.92  |   16.3x |      198.6 |    7.1 MB |   122.0 MB |

## e2e — commonmark → html

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     3.40 ms ± 0.18  |    57.82 ms ± 4.13  |   17.0x |        3.8 |    4.0 MB |   108.7 MB |
| 101 KB |    10.57 ms ± 6.16  |   136.78 ms ± 6.58  |   12.9x |        9.3 |   11.5 MB |   122.8 MB |
| 1 MB   |    65.90 ms ± 3.05  |  1001.05 ms ± 14.12 |   15.2x |       15.2 |   86.8 MB |   240.8 MB |

## e2e — commonmark → latex

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     3.35 ms ± 0.26  |    58.78 ms ± 4.68  |   17.5x |        3.8 |    4.1 MB |   108.2 MB |
| 101 KB |     8.56 ms ± 0.20  |   166.88 ms ± 98.27 |   19.5x |       11.5 |   11.8 MB |   122.2 MB |
| 1 MB   |    69.61 ms ± 15.61 |  1018.11 ms ± 87.37 |   14.6x |       14.4 |   87.2 MB |   259.3 MB |

## e2e — commonmark → rst

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     3.38 ms ± 0.17  |    47.35 ms ± 5.21  |   14.0x |        3.8 |    4.1 MB |   106.9 MB |
| 101 KB |     9.44 ms ± 0.33  |   120.07 ms ± 3.16  |   12.7x |       10.5 |   11.5 MB |   122.0 MB |
| 1 MB   |    76.77 ms ± 3.09  |   868.97 ms ± 40.01 |   11.3x |       13.0 |   87.5 MB |   237.0 MB |

## e2e — commonmark → json

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     3.03 ms ± 0.22  |    45.73 ms ± 2.76  |   15.1x |        4.2 |    4.0 MB |   107.8 MB |
| 101 KB |     7.96 ms ± 0.19  |    94.80 ms ± 3.53  |   11.9x |       12.4 |   11.5 MB |   121.8 MB |
| 1 MB   |    59.26 ms ± 1.23  |   553.80 ms ± 9.05  |    9.3x |       16.9 |   91.6 MB |   225.8 MB |

## startup — commonmark → html (near-empty input)

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 27 B   |     2.11 ms ± 0.19  |    30.94 ms ± 0.75  |   14.7x |        0.0 |    2.2 MB |    41.1 MB |

## startup — commonmark → json (near-empty input)

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 27 B   |     2.07 ms ± 0.11  |    30.95 ms ± 0.76  |   15.0x |        0.0 |    2.2 MB |    39.4 MB |

## binary size

| binary | size       | ratio |
|--------|------------|-------|
| carta  |     2.6 MB |  1.0x |
| pandoc |   179.8 MB |   69x |
