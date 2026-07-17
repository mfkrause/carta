# Benchmarks — carta vs pandoc

**Reference machine:** Apple M1 Pro (10 cores), 16 GB RAM, macOS 26.5 (arm64)
**carta:** the commit introducing this file revision (v0.0.5 plus the highlighter per-step rule-clone removal), release build (`cargo build --release`)
**pandoc:** 3.10
**Driver:** hyperfine 1.20.0, warmup 3, 12 runs

## Headline

- **~8–18× faster** end-to-end across formats and sizes; up to **~37×** on individual reader/writer surfaces.
- **~20× smaller** binary (9.1 MB vs 179.8 MB).
- **~4–23× less peak memory**.

## How to read this

Both tools run with identical `-f/-t` flags; pandoc is configured so both tools produce equivalent output and do equivalent work. Times are wall-clock end-to-end (process start included). `speedup` = pandoc mean ÷ carta mean. `MB/s` is carta throughput over the actual input size. RSS is peak resident memory from a single `/usr/bin/time` run. The HTML and LaTeX targets include syntax highlighting of code blocks in both tools.

## reader — commonmark → json

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     3.32 ms ± 0.29  |    47.75 ms ± 2.45  |   14.4x |        3.8 |    4.6 MB |   107.8 MB |
| 101 KB |     5.91 ms ± 0.38  |    92.80 ms ± 2.75  |   15.7x |       16.7 |    8.2 MB |   121.8 MB |
| 1 MB   |    33.52 ms ± 1.13  |   534.95 ms ± 8.68  |   16.0x |       29.9 |   50.7 MB |   225.8 MB |

## reader — html → json

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 14 KB  |     3.29 ms ± 0.35  |    62.71 ms ± 2.35  |   19.1x |        4.3 |    4.4 MB |   108.8 MB |
| 112 KB |     6.12 ms ± 0.67  |   184.70 ms ± 3.40  |   30.2x |       17.8 |    8.2 MB |   133.7 MB |
| 1 MB   |    32.93 ms ± 0.88  |  1229.34 ms ± 11.19 |   37.3x |       33.5 |   51.6 MB |   428.8 MB |

## writer — json → html

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     5.92 ms ± 0.39  |    33.13 ms ± 1.32  |    5.6x |        2.0 |    7.3 MB |    43.7 MB |
| 113 KB |     6.52 ms ± 0.44  |    34.74 ms ± 3.60  |    5.3x |       16.9 |    7.7 MB |    62.2 MB |
| 1 MB   |    10.55 ms ± 0.40  |   116.51 ms ± 5.74  |   11.0x |      105.8 |   10.0 MB |   123.2 MB |

## writer — json → latex

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     5.93 ms ± 0.38  |    33.22 ms ± 0.97  |    5.6x |        2.0 |    7.4 MB |    41.8 MB |
| 113 KB |     6.64 ms ± 0.44  |    33.55 ms ± 2.04  |    5.1x |       16.6 |    7.8 MB |    56.6 MB |
| 1 MB   |    11.18 ms ± 0.51  |   108.06 ms ± 3.24  |    9.7x |       99.8 |   10.1 MB |   122.8 MB |

## writer — json → rst

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     3.00 ms ± 0.59  |    32.97 ms ± 1.39  |   11.0x |        4.0 |    3.7 MB |    39.5 MB |
| 113 KB |     3.38 ms ± 0.50  |    33.53 ms ± 0.93  |    9.9x |       32.6 |    4.2 MB |    41.2 MB |
| 1 MB   |     6.47 ms ± 0.37  |    93.38 ms ± 4.94  |   14.4x |      172.3 |    6.8 MB |   122.3 MB |

## writer — json → plain

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.80 ms ± 0.41  |    33.02 ms ± 1.26  |   11.8x |        4.3 |    3.6 MB |    39.8 MB |
| 113 KB |     3.39 ms ± 1.09  |    34.92 ms ± 3.46  |   10.3x |       32.5 |    4.2 MB |    62.5 MB |
| 1 MB   |     6.10 ms ± 0.54  |    97.29 ms ± 16.59 |   16.0x |      182.9 |    6.8 MB |   122.6 MB |

## writer — json → commonmark

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.97 ms ± 0.44  |    33.60 ms ± 1.03  |   11.3x |        4.1 |    3.6 MB |    41.7 MB |
| 113 KB |     3.23 ms ± 0.33  |    48.39 ms ± 1.88  |   15.0x |       34.0 |    4.3 MB |    63.3 MB |
| 1 MB   |     6.54 ms ± 0.69  |   190.22 ms ± 7.27  |   29.1x |      170.6 |    6.9 MB |   123.5 MB |

## writer — json → mediawiki

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     3.46 ms ± 1.91  |    33.10 ms ± 1.85  |    9.6x |        3.5 |    3.5 MB |    39.5 MB |
| 113 KB |     3.49 ms ± 1.33  |    33.17 ms ± 1.65  |    9.5x |       31.6 |    4.1 MB |    43.1 MB |
| 1 MB   |     6.02 ms ± 0.42  |    93.34 ms ± 3.66  |   15.5x |      185.2 |    6.7 MB |   122.2 MB |

## writer — json → native

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.88 ms ± 0.37  |    33.31 ms ± 1.41  |   11.6x |        4.2 |    3.4 MB |    38.9 MB |
| 113 KB |     3.28 ms ± 0.32  |    35.20 ms ± 4.04  |   10.7x |       33.6 |    4.2 MB |    42.7 MB |
| 1 MB   |     7.14 ms ± 0.47  |   169.41 ms ± 4.23  |   23.7x |      156.3 |    9.2 MB |   143.0 MB |

## writer — json → json

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.79 ms ± 0.30  |    33.15 ms ± 1.42  |   11.9x |        4.3 |    3.3 MB |    38.8 MB |
| 113 KB |     3.07 ms ± 0.38  |    33.30 ms ± 1.34  |   10.8x |       35.8 |    3.7 MB |    40.0 MB |
| 1 MB   |     5.40 ms ± 0.52  |    86.17 ms ± 5.84  |   16.0x |      206.7 |    6.2 MB |   122.0 MB |

## e2e — commonmark → html

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     6.92 ms ± 0.44  |    58.12 ms ± 6.55  |    8.4x |        1.8 |    8.2 MB |   109.1 MB |
| 101 KB |    12.94 ms ± 0.46  |   151.14 ms ± 5.64  |   11.7x |        7.6 |   10.9 MB |   122.1 MB |
| 1 MB   |    71.77 ms ± 1.42  |  1153.72 ms ± 17.48 |   16.1x |       13.9 |   46.0 MB |   246.1 MB |

## e2e — commonmark → latex

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     7.17 ms ± 0.43  |    53.49 ms ± 5.86  |    7.5x |        1.8 |    8.3 MB |   107.6 MB |
| 101 KB |    13.55 ms ± 0.50  |   142.16 ms ± 6.13  |   10.5x |        7.3 |   11.2 MB |   123.6 MB |
| 1 MB   |    78.94 ms ± 1.83  |  1061.24 ms ± 16.11 |   13.4x |       12.7 |   47.3 MB |   254.7 MB |

## e2e — commonmark → rst

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     3.78 ms ± 1.42  |    48.70 ms ± 2.63  |   12.9x |        3.4 |    5.1 MB |   106.9 MB |
| 101 KB |     7.38 ms ± 0.50  |   115.54 ms ± 5.65  |   15.7x |       13.4 |    8.5 MB |   121.9 MB |
| 1 MB   |    44.53 ms ± 1.20  |   811.94 ms ± 9.85  |   18.2x |       22.5 |   43.2 MB |   236.9 MB |

## e2e — commonmark → json

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     3.36 ms ± 0.32  |    48.64 ms ± 1.56  |   14.5x |        3.8 |    4.6 MB |   107.7 MB |
| 101 KB |     6.04 ms ± 0.36  |    93.01 ms ± 2.28  |   15.4x |       16.3 |    8.1 MB |   121.8 MB |
| 1 MB   |    33.84 ms ± 0.75  |   540.47 ms ± 8.53  |   16.0x |       29.6 |   48.9 MB |   225.8 MB |

## startup — commonmark → html (near-empty input)

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 27 B   |     2.76 ms ± 0.28  |    33.07 ms ± 1.44  |   12.0x |        0.0 |    3.4 MB |    41.0 MB |

## startup — commonmark → json (near-empty input)

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 27 B   |     2.84 ms ± 0.49  |    33.20 ms ± 1.52  |   11.7x |        0.0 |    3.5 MB |    39.4 MB |

## binary size

| binary | size       | ratio |
|--------|------------|-------|
| carta  |     9.1 MB |  1.0x |
| pandoc |   179.8 MB |   20x |
