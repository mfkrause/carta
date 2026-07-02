# Benchmarks — carta vs pandoc

**Reference machine:** Apple M1 Pro (10 cores), 16 GB RAM, macOS 26.5 (arm64)
**carta:** commit `8754a4b`, release build (`cargo build --release`)
**pandoc:** 3.10
**Driver:** hyperfine 1.20.0, warmup 3, 12 runs

## Headline

- **~9–20× faster** end-to-end across formats and sizes; up to **~30×** on individual reader/writer surfaces.
- **~44× smaller** binary (4.1 MB vs 179.8 MB).
- **~2–16× less peak memory**.

## How to read this

Both tools run with identical `-f/-t` flags; pandoc is configured so both tools produce equivalent output and do equivalent work. Times are wall-clock end-to-end (process start included). `speedup` = pandoc mean ÷ carta mean. `MB/s` is carta throughput over the actual input size. RSS is peak resident memory from a single `/usr/bin/time` run.

## reader — commonmark → json

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     5.00 ms ± 1.25  |    45.22 ms ± 1.82  |    9.0x |        2.5 |    4.0 MB |   107.8 MB |
| 101 KB |     7.77 ms ± 1.89  |    93.57 ms ± 3.57  |   12.0x |       12.7 |    7.7 MB |   121.7 MB |
| 1 MB   |    44.24 ms ± 0.90  |   548.92 ms ± 7.82  |   12.4x |       22.6 |   50.6 MB |   225.8 MB |

## reader — html → json

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 14 KB  |     3.33 ms ± 0.39  |    58.03 ms ± 2.93  |   17.4x |        4.2 |    3.6 MB |   108.8 MB |
| 112 KB |     7.05 ms ± 1.13  |   180.45 ms ± 2.82  |   25.6x |       15.4 |    7.8 MB |   133.8 MB |
| 1 MB   |    69.98 ms ± 2.00  |  1267.22 ms ± 24.44 |   18.1x |       15.8 |   50.8 MB |   428.8 MB |

## writer — json → html

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.21 ms ± 0.13  |    30.83 ms ± 0.50  |   13.9x |        5.4 |    2.7 MB |    41.2 MB |
| 113 KB |     2.70 ms ± 0.31  |    36.68 ms ± 5.30  |   13.6x |       40.8 |    3.1 MB |    61.8 MB |
| 1 MB   |     5.35 ms ± 0.06  |   106.72 ms ± 3.83  |   19.9x |      208.3 |    5.8 MB |   122.9 MB |

## writer — json → latex

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     3.06 ms ± 1.44  |    31.96 ms ± 2.01  |   10.5x |        3.9 |    2.7 MB |    40.2 MB |
| 113 KB |     3.61 ms ± 0.79  |    31.94 ms ± 1.28  |    8.9x |       30.5 |    3.3 MB |    44.9 MB |
| 1 MB   |     6.66 ms ± 0.54  |   103.01 ms ± 5.10  |   15.5x |      167.4 |    6.0 MB |   122.5 MB |

## writer — json → rst

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     3.09 ms ± 0.90  |    31.99 ms ± 1.57  |   10.3x |        3.9 |    2.8 MB |    39.6 MB |
| 113 KB |     3.59 ms ± 0.93  |    32.10 ms ± 1.10  |    8.9x |       30.6 |    3.4 MB |    41.2 MB |
| 1 MB   |     6.87 ms ± 0.51  |    94.69 ms ± 2.18  |   13.8x |      162.4 |    5.7 MB |   122.3 MB |

## writer — json → plain

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.58 ms ± 0.30  |    32.44 ms ± 1.90  |   12.6x |        4.7 |    2.8 MB |    39.8 MB |
| 113 KB |     3.54 ms ± 0.99  |    35.49 ms ± 5.27  |   10.0x |       31.1 |    3.4 MB |    62.5 MB |
| 1 MB   |     6.63 ms ± 1.27  |    97.40 ms ± 4.28  |   14.7x |      168.2 |    5.8 MB |   122.7 MB |

## writer — json → commonmark

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.80 ms ± 1.01  |    32.12 ms ± 1.08  |   11.5x |        4.3 |    2.8 MB |    41.8 MB |
| 113 KB |     3.35 ms ± 0.89  |    44.24 ms ± 0.69  |   13.2x |       32.8 |    3.4 MB |    63.4 MB |
| 1 MB   |     6.43 ms ± 0.83  |   193.22 ms ± 14.03 |   30.0x |      173.4 |    6.0 MB |   123.5 MB |

## writer — json → mediawiki

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.47 ms ± 0.97  |    31.53 ms ± 1.75  |   12.7x |        4.9 |    2.7 MB |    39.5 MB |
| 113 KB |     2.93 ms ± 0.87  |    32.11 ms ± 1.83  |   11.0x |       37.6 |    3.3 MB |    43.0 MB |
| 1 MB   |     6.47 ms ± 1.63  |    94.96 ms ± 3.21  |   14.7x |      172.4 |    5.9 MB |   122.2 MB |

## writer — json → native

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.66 ms ± 0.65  |    31.63 ms ± 1.64  |   11.9x |        4.5 |    2.7 MB |    39.0 MB |
| 113 KB |     3.24 ms ± 1.03  |    38.32 ms ± 5.50  |   11.8x |       33.9 |    3.4 MB |    42.7 MB |
| 1 MB   |     7.58 ms ± 1.68  |   174.74 ms ± 6.65  |   23.0x |      147.1 |    8.4 MB |   143.1 MB |

## writer — json → json

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.49 ms ± 0.78  |    31.98 ms ± 1.82  |   12.8x |        4.8 |    2.6 MB |    38.8 MB |
| 113 KB |     2.75 ms ± 0.82  |    32.81 ms ± 0.90  |   11.9x |       40.0 |    3.0 MB |    40.0 MB |
| 1 MB   |     5.66 ms ± 1.04  |    86.19 ms ± 5.52  |   15.2x |      197.3 |    5.5 MB |   121.9 MB |

## e2e — commonmark → html

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     3.92 ms ± 0.75  |    57.12 ms ± 2.74  |   14.6x |        3.3 |    4.0 MB |   108.7 MB |
| 101 KB |     7.97 ms ± 0.51  |   133.47 ms ± 4.44  |   16.7x |       12.4 |    7.6 MB |   122.8 MB |
| 1 MB   |    51.74 ms ± 1.76  |   972.46 ms ± 42.71 |   18.8x |       19.3 |   44.9 MB |   240.8 MB |

## e2e — commonmark → latex

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     2.95 ms ± 0.03  |    51.01 ms ± 5.40  |   17.3x |        4.3 |    4.2 MB |   108.2 MB |
| 101 KB |     7.03 ms ± 0.20  |   121.22 ms ± 5.30  |   17.2x |       14.0 |    7.7 MB |   122.2 MB |
| 1 MB   |    48.53 ms ± 0.86  |   988.23 ms ± 55.04 |   20.4x |       20.6 |   44.4 MB |   259.3 MB |

## e2e — commonmark → rst

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     4.12 ms ± 0.43  |    56.77 ms ± 7.68  |   13.8x |        3.1 |    4.2 MB |   106.9 MB |
| 101 KB |     9.58 ms ± 0.76  |   126.38 ms ± 8.53  |   13.2x |       10.3 |    7.7 MB |   122.0 MB |
| 1 MB   |    65.68 ms ± 2.86  |   911.57 ms ± 55.60 |   13.9x |       15.2 |   44.6 MB |   236.9 MB |

## e2e — commonmark → json

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     4.14 ms ± 0.77  |    44.05 ms ± 0.80  |   10.6x |        3.1 |    4.0 MB |   107.8 MB |
| 101 KB |     6.92 ms ± 0.68  |    94.01 ms ± 1.70  |   13.6x |       14.3 |    7.6 MB |   121.8 MB |
| 1 MB   |    46.15 ms ± 1.91  |   587.53 ms ± 36.36 |   12.7x |       21.7 |   50.8 MB |   225.8 MB |

## startup — commonmark → html (near-empty input)

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 27 B   |     2.66 ms ± 0.54  |    32.14 ms ± 1.43  |   12.1x |        0.0 |    2.6 MB |    41.1 MB |

## startup — commonmark → json (near-empty input)

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 27 B   |     2.93 ms ± 0.34  |    32.38 ms ± 1.22  |   11.0x |        0.0 |    2.6 MB |    39.4 MB |

## binary size

| binary | size       | ratio |
|--------|------------|-------|
| carta  |     4.1 MB |  1.0x |
| pandoc |   179.8 MB |   44x |
