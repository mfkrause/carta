# Benchmarks: carta vs pandoc

Measured on an Apple M1 Pro (10 cores), 16 GB RAM, macOS 26.5 (arm64): carta 0.0.8 against
pandoc 3.10, driven by hyperfine 1.20.0 (warmup 3, 12 runs).

## Headline

carta is ~11–30× faster end-to-end across formats and sizes, and up to ~44× on individual
reader/writer surfaces. Its binary is ~21× smaller (8.5 MB vs 179.8 MB), and it uses
~4–24× less peak memory.

## How to read this

Both tools run with identical `-f/-t` flags; pandoc is configured so both tools produce equivalent output and do equivalent work. Times are wall-clock end-to-end (process start included). `speedup` = pandoc mean ÷ carta mean. `MB/s` is carta throughput over the actual input size. RSS is peak resident memory from a single `/usr/bin/time` run. The HTML and LaTeX targets include syntax highlighting of code blocks in both tools.

## reader: commonmark → json

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     3.08 ms ± 0.28  |    42.83 ms ± 1.00  |   13.9x |        4.1 |    4.6 MB |   107.8 MB |
| 101 KB |     4.75 ms ± 0.18  |    88.24 ms ± 4.94  |   18.6x |       20.8 |    8.2 MB |   121.8 MB |
| 1 MB   |    23.32 ms ± 1.02  |   511.71 ms ± 3.80  |   21.9x |       42.9 |   48.8 MB |   225.8 MB |

## reader: html → json

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 19 KB  |     3.36 ms ± 0.30  |    55.40 ms ± 2.63  |   16.5x |        5.4 |    4.5 MB |   109.8 MB |
| 145 KB |     5.57 ms ± 0.70  |   217.64 ms ± 26.51 |   39.1x |       25.4 |    9.2 MB |   149.8 MB |
| 1 MB   |    30.67 ms ± 1.54  |  1350.65 ms ± 20.17 |   44.0x |       46.8 |   57.3 MB |   467.8 MB |

## writer: json → html

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     4.40 ms ± 0.62  |    30.29 ms ± 0.81  |    6.9x |        2.7 |    6.0 MB |    43.7 MB |
| 113 KB |     4.35 ms ± 0.39  |    36.23 ms ± 6.33  |    8.3x |       25.3 |    6.3 MB |    62.3 MB |
| 1 MB   |     6.76 ms ± 0.45  |   115.02 ms ± 7.31  |   17.0x |      165.0 |    8.6 MB |   123.2 MB |

## writer: json → latex

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     4.63 ms ± 0.57  |    30.55 ms ± 2.83  |    6.6x |        2.6 |    6.1 MB |    41.8 MB |
| 113 KB |     4.35 ms ± 0.18  |    30.87 ms ± 2.45  |    7.1x |       25.3 |    6.6 MB |    56.6 MB |
| 1 MB   |     7.78 ms ± 0.55  |   105.44 ms ± 3.89  |   13.5x |      143.3 |    8.9 MB |   122.8 MB |

## writer: json → rst

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.78 ms ± 0.45  |    29.61 ms ± 0.98  |   10.6x |        4.3 |    3.5 MB |    39.6 MB |
| 113 KB |     3.34 ms ± 0.52  |    30.01 ms ± 0.79  |    9.0x |       33.0 |    4.0 MB |    41.2 MB |
| 1 MB   |     5.78 ms ± 0.44  |    92.16 ms ± 3.96  |   16.0x |      193.2 |    6.5 MB |   122.3 MB |

## writer: json → plain

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.92 ms ± 0.44  |    30.64 ms ± 1.12  |   10.5x |        4.1 |    3.5 MB |    39.8 MB |
| 113 KB |     3.03 ms ± 0.39  |    30.29 ms ± 1.24  |   10.0x |       36.3 |    4.1 MB |    62.6 MB |
| 1 MB   |     5.29 ms ± 0.11  |    92.59 ms ± 1.60  |   17.5x |      211.0 |    6.6 MB |   122.7 MB |

## writer: json → commonmark

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.69 ms ± 0.27  |    30.01 ms ± 0.65  |   11.1x |        4.5 |    3.5 MB |    41.8 MB |
| 113 KB |     3.34 ms ± 0.50  |    42.86 ms ± 0.93  |   12.8x |       33.0 |    4.1 MB |    63.3 MB |
| 1 MB   |     5.51 ms ± 0.34  |   179.83 ms ± 1.71  |   32.6x |      202.4 |    6.8 MB |   123.5 MB |

## writer: json → mediawiki

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.72 ms ± 0.35  |    30.46 ms ± 1.25  |   11.2x |        4.4 |    3.5 MB |    39.5 MB |
| 113 KB |     2.90 ms ± 0.11  |    30.34 ms ± 0.79  |   10.5x |       37.9 |    4.0 MB |    43.1 MB |
| 1 MB   |     5.43 ms ± 0.56  |    94.08 ms ± 3.71  |   17.3x |      205.4 |    6.7 MB |   122.2 MB |

## writer: json → native

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.95 ms ± 0.49  |    30.44 ms ± 0.88  |   10.3x |        4.1 |    3.4 MB |    38.9 MB |
| 113 KB |     3.12 ms ± 0.38  |    32.67 ms ± 4.80  |   10.5x |       35.3 |    4.1 MB |    42.7 MB |
| 1 MB   |     6.33 ms ± 0.19  |   162.15 ms ± 5.01  |   25.6x |      176.3 |    9.0 MB |   143.1 MB |

## writer: json → json

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.74 ms ± 0.41  |    30.02 ms ± 0.82  |   11.0x |        4.4 |    3.2 MB |    38.8 MB |
| 113 KB |     2.95 ms ± 0.43  |    30.54 ms ± 1.09  |   10.4x |       37.4 |    3.6 MB |    40.0 MB |
| 1 MB   |     4.68 ms ± 0.33  |    80.46 ms ± 1.48  |   17.2x |      238.4 |    6.2 MB |   121.9 MB |

## e2e: commonmark → html

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     5.14 ms ± 0.35  |    55.52 ms ± 1.01  |   10.8x |        2.5 |    7.4 MB |   109.2 MB |
| 101 KB |     7.84 ms ± 0.37  |   143.69 ms ± 3.92  |   18.3x |       12.6 |   10.1 MB |   122.1 MB |
| 1 MB   |    36.86 ms ± 1.00  |  1112.89 ms ± 6.96  |   30.2x |       27.2 |   44.7 MB |   246.2 MB |

## e2e: commonmark → latex

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     5.12 ms ± 0.23  |    55.18 ms ± 0.86  |   10.8x |        2.5 |    7.4 MB |   107.6 MB |
| 101 KB |     8.61 ms ± 0.15  |   133.98 ms ± 5.36  |   15.6x |       11.5 |   10.4 MB |   123.7 MB |
| 1 MB   |    46.26 ms ± 0.87  |  1018.98 ms ± 7.95  |   22.0x |       21.6 |   47.1 MB |   254.7 MB |

## e2e: commonmark → rst

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     3.23 ms ± 0.20  |    42.77 ms ± 0.72  |   13.3x |        4.0 |    4.9 MB |   106.9 MB |
| 101 KB |     6.11 ms ± 0.32  |   114.90 ms ± 10.54 |   18.8x |       16.2 |    8.3 MB |   122.0 MB |
| 1 MB   |    36.15 ms ± 1.04  |   781.94 ms ± 9.48  |   21.6x |       27.7 |   42.9 MB |   237.0 MB |

## e2e: commonmark → json

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     3.37 ms ± 0.60  |    43.05 ms ± 0.82  |   12.8x |        3.8 |    4.6 MB |   107.8 MB |
| 101 KB |     5.19 ms ± 0.83  |    92.77 ms ± 3.24  |   17.9x |       19.0 |    8.1 MB |   121.8 MB |
| 1 MB   |    23.66 ms ± 0.63  |   526.10 ms ± 11.13 |   22.2x |       42.3 |   48.8 MB |   225.8 MB |

## startup: commonmark → html (near-empty input)

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 27 B   |     2.53 ms ± 0.15  |    29.84 ms ± 0.54  |   11.8x |        0.0 |    3.5 MB |    41.1 MB |

## startup: commonmark → json (near-empty input)

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 27 B   |     2.53 ms ± 0.33  |    30.25 ms ± 0.99  |   12.0x |        0.0 |    3.5 MB |    39.4 MB |

## binary size

| binary | size       | ratio |
|--------|------------|-------|
| carta  |     8.5 MB |  1.0x |
| pandoc |   179.8 MB |   21x |
