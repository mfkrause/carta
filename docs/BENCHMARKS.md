# Benchmarks — carta vs pandoc

- **Reference machine:** Apple M1 Pro (10 cores), 16 GB RAM, macOS 26.5 (arm64)
- **carta:** 0.0.6
- **pandoc:** 3.10
- **Driver:** hyperfine 1.20.0, warmup 3, 12 runs

## Headline

- **~8–17× faster** end-to-end across formats and sizes; up to **~35×** on individual reader/writer surfaces.
- **~20× smaller** binary (9.1 MB vs 179.8 MB).
- **~4–25× less peak memory**.

## How to read this

Both tools run with identical `-f/-t` flags; pandoc is configured so both tools produce equivalent output and do equivalent work. Times are wall-clock end-to-end (process start included). `speedup` = pandoc mean ÷ carta mean. `MB/s` is carta throughput over the actual input size. RSS is peak resident memory from a single `/usr/bin/time` run. The HTML and LaTeX targets include syntax highlighting of code blocks in both tools.

## reader — commonmark → json

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     3.67 ms ± 0.75  |    48.95 ms ± 5.18  |   13.3x |        3.5 |    4.5 MB |   107.8 MB |
| 101 KB |     6.77 ms ± 1.30  |    95.74 ms ± 5.68  |   14.1x |       14.6 |    8.1 MB |   121.8 MB |
| 1 MB   |    36.48 ms ± 2.91  |   554.93 ms ± 11.72 |   15.2x |       27.4 |   48.9 MB |   225.8 MB |

## reader — html → json

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 14 KB  |     3.51 ms ± 0.73  |    60.07 ms ± 5.70  |   17.1x |        4.0 |    4.3 MB |   108.8 MB |
| 112 KB |     6.11 ms ± 1.22  |   175.75 ms ± 5.31  |   28.8x |       17.8 |    8.1 MB |   133.8 MB |
| 1 MB   |    36.18 ms ± 3.71  |  1265.50 ms ± 27.90 |   35.0x |       30.5 |   52.5 MB |   428.8 MB |

## writer — json → html

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     6.33 ms ± 1.01  |    31.53 ms ± 1.34  |    5.0x |        1.9 |    7.3 MB |    43.7 MB |
| 113 KB |     6.83 ms ± 0.53  |    42.46 ms ± 3.31  |    6.2x |       16.1 |    7.5 MB |    62.2 MB |
| 1 MB   |    11.18 ms ± 0.56  |   116.77 ms ± 4.17  |   10.4x |       99.8 |    9.9 MB |   123.2 MB |

## writer — json → latex

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     6.26 ms ± 0.45  |    30.86 ms ± 1.23  |    4.9x |        1.9 |    7.3 MB |    41.8 MB |
| 113 KB |     6.69 ms ± 0.42  |    34.27 ms ± 5.17  |    5.1x |       16.4 |    7.7 MB |    56.6 MB |
| 1 MB   |    11.22 ms ± 0.56  |   106.82 ms ± 4.85  |    9.5x |       99.4 |   10.1 MB |   122.8 MB |

## writer — json → rst

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.86 ms ± 0.30  |    30.56 ms ± 1.13  |   10.7x |        4.2 |    3.5 MB |    39.6 MB |
| 113 KB |     3.29 ms ± 0.26  |    30.96 ms ± 1.90  |    9.4x |       33.5 |    4.1 MB |    41.2 MB |
| 1 MB   |     6.49 ms ± 0.45  |    91.60 ms ± 3.67  |   14.1x |      172.0 |    6.5 MB |   122.3 MB |

## writer — json → plain

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.91 ms ± 0.40  |    30.46 ms ± 1.17  |   10.5x |        4.1 |    3.5 MB |    39.8 MB |
| 113 KB |     3.19 ms ± 0.21  |    35.02 ms ± 5.41  |   11.0x |       34.5 |    4.2 MB |    62.5 MB |
| 1 MB   |     7.07 ms ± 1.86  |   104.36 ms ± 6.82  |   14.8x |      157.8 |    6.6 MB |   122.6 MB |

## writer — json → commonmark

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     3.28 ms ± 1.64  |    30.83 ms ± 1.12  |    9.4x |        3.7 |    3.6 MB |    41.7 MB |
| 113 KB |     3.51 ms ± 0.55  |    43.86 ms ± 2.51  |   12.5x |       31.4 |    4.2 MB |    63.3 MB |
| 1 MB   |     6.52 ms ± 0.45  |   190.11 ms ± 9.68  |   29.1x |      171.0 |    6.9 MB |   123.6 MB |

## writer — json → mediawiki

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     2.87 ms ± 0.36  |    30.64 ms ± 1.04  |   10.7x |        4.2 |    3.5 MB |    39.5 MB |
| 113 KB |     3.30 ms ± 0.48  |    31.82 ms ± 1.21  |    9.6x |       33.4 |    4.0 MB |    43.1 MB |
| 1 MB   |     6.47 ms ± 0.87  |    99.52 ms ± 5.82  |   15.4x |      172.3 |    6.6 MB |   122.2 MB |

## writer — json → native

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     3.28 ms ± 0.78  |    31.64 ms ± 1.31  |    9.7x |        3.7 |    3.4 MB |    39.0 MB |
| 113 KB |     3.64 ms ± 0.51  |    42.31 ms ± 4.47  |   11.6x |       30.2 |    4.1 MB |    42.7 MB |
| 1 MB   |     7.43 ms ± 0.62  |   185.86 ms ± 17.03 |   25.0x |      150.1 |    9.2 MB |   143.1 MB |

## writer — json → json

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 12 KB  |     3.32 ms ± 0.78  |    33.11 ms ± 5.95  |   10.0x |        3.6 |    3.3 MB |    38.8 MB |
| 113 KB |     3.82 ms ± 3.45  |    31.60 ms ± 1.20  |    8.3x |       28.8 |    3.7 MB |    40.0 MB |
| 1 MB   |     6.00 ms ± 0.74  |    91.19 ms ± 2.44  |   15.2x |      186.1 |    6.2 MB |   122.0 MB |

## e2e — commonmark → html

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     7.26 ms ± 0.48  |    56.32 ms ± 3.31  |    7.8x |        1.8 |    8.0 MB |   109.1 MB |
| 101 KB |    13.01 ms ± 0.57  |   157.09 ms ± 6.25  |   12.1x |        7.6 |   10.8 MB |   122.1 MB |
| 1 MB   |    74.47 ms ± 2.11  |  1177.96 ms ± 22.95 |   15.8x |       13.4 |   47.1 MB |   246.1 MB |

## e2e — commonmark → latex

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     7.19 ms ± 0.36  |    54.32 ms ± 1.96  |    7.6x |        1.8 |    8.2 MB |   107.7 MB |
| 101 KB |    13.77 ms ± 0.37  |   144.11 ms ± 5.49  |   10.5x |        7.2 |   11.2 MB |   123.6 MB |
| 1 MB   |    89.49 ms ± 10.30 |  1161.50 ms ± 145.53 |   13.0x |       11.2 |   48.6 MB |   254.7 MB |

## e2e — commonmark → rst

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     4.17 ms ± 0.65  |    56.43 ms ± 2.34  |   13.5x |        3.1 |    4.9 MB |   106.9 MB |
| 101 KB |     8.23 ms ± 1.18  |   117.73 ms ± 5.53  |   14.3x |       12.0 |    8.2 MB |   121.9 MB |
| 1 MB   |    48.54 ms ± 1.48  |   837.98 ms ± 17.81 |   17.3x |       20.6 |   44.0 MB |   237.0 MB |

## e2e — commonmark → json

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 13 KB  |     3.59 ms ± 0.49  |    44.72 ms ± 3.39  |   12.5x |        3.6 |    4.5 MB |   107.8 MB |
| 101 KB |     6.14 ms ± 0.56  |    92.53 ms ± 3.98  |   15.1x |       16.1 |    8.1 MB |   121.8 MB |
| 1 MB   |    34.32 ms ± 1.53  |   547.04 ms ± 9.21  |   15.9x |       29.2 |   48.8 MB |   225.8 MB |

## startup — commonmark → html (near-empty input)

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 27 B   |     2.72 ms ± 0.28  |    30.96 ms ± 1.07  |   11.4x |        0.0 |    3.3 MB |    41.0 MB |

## startup — commonmark → json (near-empty input)

| size   | carta mean ± σ      | pandoc mean ± σ     | speedup | carta MB/s | carta RSS | pandoc RSS |
|--------|---------------------|---------------------|---------|------------|-----------|------------|
| 27 B   |     2.72 ms ± 0.29  |    30.65 ms ± 0.99  |   11.3x |        0.0 |    3.4 MB |    39.4 MB |

## binary size

| binary | size       | ratio |
|--------|------------|-------|
| carta  |     9.1 MB |  1.0x |
| pandoc |   179.8 MB |   20x |
