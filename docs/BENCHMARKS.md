# Benchmarks — carta vs pandoc

**Reference machine:** Apple M1 Pro (10 cores), 16 GB RAM, macOS 26.5 (arm64)
**carta:** the commit introducing this file revision (v0.0.5 plus the keyed math symbol lookups, the streaming fill sink, and the mediawiki/markdown reader scan bounds), release build (`cargo build --release`)
**pandoc:** 3.10
**Driver:** hyperfine 1.20.0, warmup 3, 12 runs

## Headline

- **~8–18× faster** end-to-end across formats and sizes; up to **~36×** on individual reader/writer surfaces.
- **~20× smaller** binary (9.1 MB vs 179.8 MB).
- **~4–25× less peak memory**.

## How to read this

Both tools run with identical `-f/-t` flags; pandoc is configured so both tools produce equivalent output and do equivalent work. Times are wall-clock end-to-end (process start included). `speedup` = pandoc mean ÷ carta mean. `MB/s` is carta throughput over the actual input size. RSS is peak resident memory from a single `/usr/bin/time` run. The HTML and LaTeX targets include syntax highlighting of code blocks in both tools.

## reader — commonmark → json

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     3.06 ms ± 0.24  |    48.95 ms ± 0.95  |   16.0x |        4.2 |    4.5 MB |   107.8 MB |
| 101 KB |     6.06 ms ± 0.56  |    92.67 ms ± 4.28  |   15.3x |       16.3 |    8.1 MB |   121.8 MB |
| 1 MB   |    33.30 ms ± 0.98  |   534.02 ms ± 12.80 |   16.0x |       30.1 |   48.9 MB |   225.8 MB |

## reader — html → json

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 14 KB  |     3.10 ms ± 0.38  |    56.26 ms ± 0.73  |   18.2x |        4.5 |    4.4 MB |   108.8 MB |
| 112 KB |     5.43 ms ± 0.20  |   170.93 ms ± 4.53  |   31.5x |       20.0 |    8.2 MB |   133.8 MB |
| 1 MB   |    33.23 ms ± 2.03  |  1199.26 ms ± 26.58 |   36.1x |       33.2 |   51.6 MB |   428.8 MB |

## writer — json → html

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     5.70 ms ± 0.41  |    31.13 ms ± 1.26  |    5.5x |        2.1 |    7.2 MB |    43.7 MB |
| 113 KB |     5.98 ms ± 0.23  |    35.48 ms ± 6.22  |    5.9x |       18.4 |    7.7 MB |    62.2 MB |
| 1 MB   |    10.38 ms ± 0.67  |   110.83 ms ± 6.56  |   10.7x |      107.5 |    9.9 MB |   123.2 MB |

## writer — json → latex

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     5.42 ms ± 0.13  |    30.30 ms ± 0.54  |    5.6x |        2.2 |    7.3 MB |    41.9 MB |
| 113 KB |     6.08 ms ± 0.26  |    30.25 ms ± 0.54  |    5.0x |       18.1 |    7.8 MB |    56.6 MB |
| 1 MB   |    10.45 ms ± 0.24  |   106.16 ms ± 1.37  |   10.2x |      106.7 |   10.0 MB |   122.8 MB |

## writer — json → rst

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.53 ms ± 0.15  |    30.56 ms ± 3.11  |   12.1x |        4.8 |    3.6 MB |    39.6 MB |
| 113 KB |     3.02 ms ± 0.29  |    31.04 ms ± 3.99  |   10.3x |       36.4 |    4.1 MB |    41.2 MB |
| 1 MB   |     6.01 ms ± 0.37  |    93.84 ms ± 1.22  |   15.6x |      185.6 |    6.6 MB |   122.3 MB |

## writer — json → plain

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.55 ms ± 0.23  |    30.39 ms ± 0.47  |   11.9x |        4.7 |    3.5 MB |    39.8 MB |
| 113 KB |     3.23 ms ± 0.67  |    30.66 ms ± 1.66  |    9.5x |       34.1 |    4.2 MB |    62.5 MB |
| 1 MB   |     5.80 ms ± 0.37  |    94.02 ms ± 1.98  |   16.2x |      192.3 |    6.6 MB |   122.6 MB |

## writer — json → commonmark

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.53 ms ± 0.13  |    30.69 ms ± 2.97  |   12.1x |        4.8 |    3.6 MB |    41.7 MB |
| 113 KB |     3.09 ms ± 0.22  |    43.90 ms ± 0.61  |   14.2x |       35.6 |    4.2 MB |    63.3 MB |
| 1 MB   |     5.85 ms ± 0.27  |   185.82 ms ± 5.53  |   31.8x |      190.7 |    6.9 MB |   123.5 MB |

## writer — json → mediawiki

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.68 ms ± 0.28  |    30.96 ms ± 1.04  |   11.6x |        4.5 |    3.5 MB |    39.6 MB |
| 113 KB |     3.02 ms ± 0.49  |    30.81 ms ± 1.56  |   10.2x |       36.5 |    4.0 MB |    43.1 MB |
| 1 MB   |     5.49 ms ± 0.30  |    94.42 ms ± 2.34  |   17.2x |      203.3 |    6.7 MB |   122.2 MB |

## writer — json → native

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.53 ms ± 0.12  |    30.78 ms ± 0.68  |   12.2x |        4.8 |    3.4 MB |    38.9 MB |
| 113 KB |     3.02 ms ± 0.35  |    34.19 ms ± 5.65  |   11.3x |       36.4 |    4.1 MB |    42.7 MB |
| 1 MB   |     6.51 ms ± 0.24  |   161.88 ms ± 5.68  |   24.9x |      171.3 |    9.0 MB |   143.1 MB |

## writer — json → json

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.63 ms ± 0.23  |    30.72 ms ± 0.91  |   11.7x |        4.6 |    3.3 MB |    38.8 MB |
| 113 KB |     2.76 ms ± 0.16  |    31.52 ms ± 1.29  |   11.4x |       39.9 |    3.7 MB |    40.0 MB |
| 1 MB   |     4.83 ms ± 0.15  |    82.32 ms ± 3.49  |   17.0x |      231.0 |    6.2 MB |   121.9 MB |

## e2e — commonmark → html

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     6.41 ms ± 0.23  |    56.41 ms ± 0.57  |    8.8x |        2.0 |    8.1 MB |   109.1 MB |
| 101 KB |    11.82 ms ± 0.08  |   145.76 ms ± 5.31  |   12.3x |        8.4 |   10.9 MB |   122.1 MB |
| 1 MB   |    68.16 ms ± 1.36  |  1095.15 ms ± 14.10 |   16.1x |       14.7 |   45.9 MB |   246.2 MB |

## e2e — commonmark → latex

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     6.96 ms ± 0.27  |    50.37 ms ± 4.88  |    7.2x |        1.8 |    8.2 MB |   107.6 MB |
| 101 KB |    13.38 ms ± 0.45  |   136.95 ms ± 3.42  |   10.2x |        7.4 |   11.2 MB |   123.6 MB |
| 1 MB   |    77.98 ms ± 1.97  |  1037.30 ms ± 24.03 |   13.3x |       12.8 |   47.7 MB |   254.6 MB |

## e2e — commonmark → rst

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     3.24 ms ± 0.17  |    43.22 ms ± 0.72  |   13.4x |        3.9 |    5.0 MB |   106.9 MB |
| 101 KB |     6.95 ms ± 0.22  |   109.79 ms ± 5.54  |   15.8x |       14.2 |    8.4 MB |   121.9 MB |
| 1 MB   |    44.88 ms ± 0.77  |   792.09 ms ± 20.71 |   17.7x |       22.3 |   43.2 MB |   237.0 MB |

## e2e — commonmark → json

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     3.25 ms ± 0.69  |    44.06 ms ± 1.50  |   13.5x |        3.9 |    4.5 MB |   107.8 MB |
| 101 KB |     5.53 ms ± 0.19  |    90.22 ms ± 5.46  |   16.3x |       17.9 |    8.0 MB |   121.8 MB |
| 1 MB   |    33.01 ms ± 1.49  |   513.47 ms ± 3.12  |   15.6x |       30.3 |   48.8 MB |   225.8 MB |

## startup — commonmark → html (near-empty input)

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 27 B   |     2.64 ms ± 0.14  |    32.71 ms ± 0.77  |   12.4x |        0.0 |    3.5 MB |    41.0 MB |

## startup — commonmark → json (near-empty input)

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 27 B   |     2.47 ms ± 0.17  |    32.38 ms ± 1.42  |   13.1x |        0.0 |    3.5 MB |    39.4 MB |

## binary size

| binary | size       | ratio |
|--------|------------|-------|
| carta  |     9.1 MB |  1.0x |
| pandoc |   179.8 MB |   20x |
